param(
    [Parameter(Mandatory = $true)]
    [int] $AppProcessId
)

$ErrorActionPreference = "Stop"

try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type -AssemblyName System.Windows.Forms

    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ChadexTrayInput {
    [StructLayout(LayoutKind.Sequential)]
    private struct GuiThreadInfo {
        public uint cbSize, flags;
        public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret;
        public int left, top, right, bottom;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr hwnd, StringBuilder className, int maxCount);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll")]
    private static extern bool GetGUIThreadInfo(uint threadId, ref GuiThreadInfo info);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);
    public static uint WindowProcessId(IntPtr hwnd) {
        uint processId = 0;
        return hwnd != IntPtr.Zero && GetWindowThreadProcessId(hwnd, out processId) != 0 ? processId : 0;
    }
    public static bool MenuBelongsTo(int nativeHandle, uint processId) {
        var hwnd = new IntPtr(unchecked((long)(uint)nativeHandle));
        uint menuProcessId;
        uint threadId = GetWindowThreadProcessId(hwnd, out menuProcessId);
        if (hwnd == IntPtr.Zero || threadId == 0 || menuProcessId != processId) return false;
        var info = new GuiThreadInfo();
        info.cbSize = (uint)Marshal.SizeOf(typeof(GuiThreadInfo));
        if (!GetGUIThreadInfo(threadId, ref info) || (info.flags & 0x14) == 0 ||
            WindowProcessId(info.hwndMenuOwner) != processId) return false;
        var ownerClass = new StringBuilder(256);
        return GetClassName(info.hwndMenuOwner, ownerClass, ownerClass.Capacity) != 0 &&
            ownerClass.ToString() == "tray_icon_app";
    }
}
"@

    # Pin the caller's exact process, never discover an app by name. Keep its handle
    # open so an exited process/PID reuse cannot be accepted during polling.
    $process = Get-Process -Id $AppProcessId -ErrorAction SilentlyContinue
    if ($AppProcessId -le 0 -or $null -eq $process) {
        [Console]::Out.WriteLine("tray_app_unavailable")
        exit 2
    }
    $null = $process.Handle

    # main.rs enables single-instance, but that is NOT OS-level ownership proof.
    # The pinned tray-icon 0.25.1 passes its tray_icon_app HWND to TrackPopupMenu;
    # MenuBelongsTo checks the actual active menu owner, never a guessed HWND.
    function Test-AppIdentity {
        $process.Refresh()
        return (-not $process.HasExited)
    }
    if (-not (Test-AppIdentity)) {
        [Console]::Out.WriteLine("tray_app_unavailable")
        exit 2
    }

    $desktop = [System.Windows.Automation.AutomationElement]::RootElement
    $taskbarCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::ClassNameProperty,
        "Shell_TrayWnd"
    )
    $taskbar = $desktop.FindFirst(
        [System.Windows.Automation.TreeScope]::Children,
        $taskbarCondition
    )
    if ($null -eq $taskbar) {
        [Console]::Out.WriteLine("tray_taskbar_unavailable")
        exit 2
    }

    $shellProcessId = $taskbar.Current.ProcessId
    function Get-NotificationScopes {
        # Never search all Shell_TrayWnd for Chadex: it also contains app buttons.
        foreach ($className in @("TrayNotifyWnd", "SystemTray.SystemTrayFrame")) {
            $areas = $taskbar.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.PropertyCondition]::new(
                    [System.Windows.Automation.AutomationElement]::ClassNameProperty, $className)
            )
            foreach ($area in $areas) {
                if ($area.Current.ProcessId -eq $shellProcessId) { $area }
            }
        }
    }
    function Get-TrayScopes {
        Get-NotificationScopes
        foreach ($className in @("NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland")) {
            $overflows = $desktop.FindAll(
                [System.Windows.Automation.TreeScope]::Children,
                [System.Windows.Automation.PropertyCondition]::new(
                    [System.Windows.Automation.AutomationElement]::ClassNameProperty, $className)
            )
            foreach ($overflow in $overflows) {
                if ($overflow.Current.ProcessId -eq $shellProcessId) { $overflow }
            }
        }
    }
    function Find-ChadexTrayIcon {
        $candidates = @{}
        foreach ($scope in @(Get-TrayScopes)) {
            $elements = $scope.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition
            )
            foreach ($element in $elements) {
                try {
                    $current = $element.Current
                    if ($current.Name -match '^Chadex(?:\s|$)' -and
                        $current.ProcessId -eq $shellProcessId -and
                        -not $current.IsOffscreen -and $current.IsEnabled -and
                        ($current.ControlType -eq [System.Windows.Automation.ControlType]::Button -or
                         $current.ControlType -eq [System.Windows.Automation.ControlType]::Custom)) {
                        $candidates[($element.GetRuntimeId() -join ":")] = $element
                    }
                } catch [System.Windows.Automation.ElementNotAvailableException] {
                    # Explorer can rebuild its notification elements while polling.
                }
            }
        }
        if ($candidates.Count -gt 1) { throw "tray_icon_ambiguous" }
        if ($candidates.Count -eq 1) { return @($candidates.Values)[0] }
        return $null
    }

    function Invoke-Element($element) {
        $pattern = $null
        if ($element.TryGetCurrentPattern(
            [System.Windows.Automation.InvokePattern]::Pattern,
            [ref]$pattern
        )) {
            $pattern.Invoke()
            return
        }
        $pattern = $null
        if ($element.TryGetCurrentPattern(
            [System.Windows.Automation.LegacyIAccessiblePattern]::Pattern,
            [ref]$pattern
        )) {
            $pattern.DoDefaultAction()
            return
        }
        throw "tray_element_not_invokable"
    }

    function Test-MenuOwner($menu) {
        return ((Test-AppIdentity) -and $menu.Current.ProcessId -eq $AppProcessId -and
            -not $menu.Current.IsOffscreen -and
            [ChadexTrayInput]::MenuBelongsTo($menu.Current.NativeWindowHandle, $AppProcessId))
    }
    function Find-ChadexMenu {
        $elements = $desktop.FindAll(
            [System.Windows.Automation.TreeScope]::Children,
            [System.Windows.Automation.PropertyCondition]::new(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::Menu)
        )
        $menuCandidates = @()
        foreach ($element in $elements) {
            try {
                if (-not (Test-MenuOwner $element)) { continue }
                $items = $element.FindAll(
                    [System.Windows.Automation.TreeScope]::Children,
                    [System.Windows.Automation.PropertyCondition]::new(
                        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                        [System.Windows.Automation.ControlType]::MenuItem)
                )
                $showItems = @($items | Where-Object { $_.Current.Name -ceq "Show Chadex" })
                $quitItems = @($items | Where-Object { $_.Current.Name -ceq "Quit" })
                if ($showItems.Count -eq 1 -and $quitItems.Count -eq 1 -and
                    $quitItems[0].Current.ProcessId -eq $AppProcessId -and
                    $quitItems[0].Current.IsEnabled -and -not $quitItems[0].Current.IsOffscreen) {
                    $menuCandidates += [PSCustomObject]@{ Menu = $element; Quit = $quitItems[0] }
                }
            } catch [System.Windows.Automation.ElementNotAvailableException] {
                # A closing/replaced menu is not identity evidence.
            }
        }
        if ($menuCandidates.Count -gt 1) { throw "tray_menu_ambiguous" }
        if ($menuCandidates.Count -eq 1) { return $menuCandidates[0] }
        return $null
    }
    function Wait-ChadexMenu {
        $wait = [System.Diagnostics.Stopwatch]::StartNew()
        do {
            $result = $null
            try {
                $result = Find-ChadexMenu
            } catch [System.Windows.Automation.ElementNotAvailableException] {
                # Retry discovery, never act on a stale UIA element.
            }
            if ($null -ne $result) { return $result }
            Start-Sleep -Milliseconds 100
        } while ($wait.ElapsedMilliseconds -lt 2000)
        return $null
    }

    $icon = $null
    $overflowOpened = $false
    $discovery = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        if (-not (Test-AppIdentity)) { throw "tray_owner_unavailable" }
        try {
            $icon = Find-ChadexTrayIcon
            if ($null -ne $icon) { break }
            if (-not $overflowOpened) {
                $toggles = @{}
                foreach ($area in @(Get-NotificationScopes)) {
                    $buttons = $area.FindAll(
                        [System.Windows.Automation.TreeScope]::Descendants,
                        [System.Windows.Automation.PropertyCondition]::new(
                            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                            [System.Windows.Automation.ControlType]::Button)
                    )
                    foreach ($button in $buttons) {
                        $current = $button.Current
                        # Win10's chevron control ID and Win11's class avoid localized names.
                        if (($current.AutomationId -ceq "1502" -or
                             $current.ClassName -ceq "SystemTray.ChevronIconView") -and
                            $current.ProcessId -eq $shellProcessId -and
                            $current.IsEnabled -and -not $current.IsOffscreen) {
                            $toggles[($button.GetRuntimeId() -join ":")] = $button
                        }
                    }
                }
                if ($toggles.Count -gt 1) { throw "tray_overflow_ambiguous" }
                if ($toggles.Count -eq 1) {
                    # Do not toggle again if Explorer invalidates the element on invoke.
                    $overflowOpened = $true
                    Invoke-Element (@($toggles.Values)[0])
                }
            }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # Re-enumerate notification/overflow roots until the same deadline.
        }
        Start-Sleep -Milliseconds 100
    } while ($discovery.ElapsedMilliseconds -lt 4000)
    if ($null -eq $icon) {
        [Console]::Out.WriteLine("tray_icon_unavailable")
        exit 2
    }

    # Explorer owns the accessible icon; UIA does not expose its NOTIFYICONDATA
    # callback HWND/uID. Name + scope is only a candidate, not AppProcessId proof.
    # Opening it must be followed by the independent native menu-owner check.
    try {
        if (-not (Test-AppIdentity)) { throw "tray_owner_unavailable" }
        $icon.SetFocus()
        if (-not [System.Windows.Automation.Automation]::Compare(
            $icon, [System.Windows.Automation.AutomationElement]::FocusedElement)) {
            throw "tray_focus_unavailable"
        }
        [System.Windows.Forms.SendKeys]::SendWait("+{F10}")
    } catch {
        # Focus/keyboard failure must still reach the right-click fallback.
    }

    $selection = Wait-ChadexMenu
    if ($null -eq $selection) {
        # Re-resolve the scoped, unique candidate after any shell layout change.
        $icon = Find-ChadexTrayIcon
        if ($null -eq $icon -or -not (Test-AppIdentity)) {
            [Console]::Out.WriteLine("tray_menu_unavailable")
            exit 3
        }
        $bounds = $icon.Current.BoundingRectangle
        if ($bounds.IsEmpty -or $bounds.Width -lt 2 -or $bounds.Height -lt 2) {
            [Console]::Out.WriteLine("tray_menu_unavailable")
            exit 3
        }
        $x = [int]($bounds.Left + ($bounds.Width / 2))
        $y = [int]($bounds.Top + ($bounds.Height / 2))
        if (-not [ChadexTrayInput]::SetCursorPos($x, $y)) {
            [Console]::Out.WriteLine("tray_menu_unavailable")
            exit 3
        }
        [ChadexTrayInput]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 50
        [ChadexTrayInput]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)
        $selection = Wait-ChadexMenu
        if ($null -eq $selection) {
            [Console]::Out.WriteLine("tray_menu_unavailable")
            exit 3
        }
    }
    # Revalidate immediately before invoking; never trust menu labels alone.
    if (-not (Test-MenuOwner $selection.Menu) -or
        $selection.Quit.Current.ProcessId -ne $AppProcessId -or
        $selection.Quit.Current.Name -cne "Quit" -or
        -not $selection.Quit.Current.IsEnabled -or $selection.Quit.Current.IsOffscreen) {
        [Console]::Out.WriteLine("tray_menu_unavailable")
        exit 3
    }
    Invoke-Element $selection.Quit
    [Console]::Out.WriteLine("tray_quit_sent")
    exit 0
} catch {
    [Console]::Out.WriteLine("tray_quit_failed")
    exit 1
}
