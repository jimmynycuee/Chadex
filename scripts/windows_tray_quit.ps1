param(
    [Parameter(Mandatory = $true)]
    [int] $AppProcessId
)

$ErrorActionPreference = "Stop"
$script:TrayDiagnostics = [ordered]@{
    taskbar_found = $false
    same_session_as_shell = $false
    notification_scope_count = 0
    overflow_scope_count = 0
    overflow_toggle_count = 0
    overflow_open_attempted = $false
    name_match_count = 0
    shell_process_match_count = 0
    visible_match_count = 0
    enabled_match_count = 0
    supported_control_match_count = 0
    candidate_count = 0
    native_window_count = 0
    native_icon_match_count = 0
    native_rect_available = $false
    native_rect_root_kind = 0
    native_cursor_matches_rect = $false
    native_click_attempted = $false
    native_click_down_input_count = 0
    native_click_up_input_count = 0
    preexisting_menu_count = 0
    menu_element_count = 0
    menu_app_process_count = 0
    menu_exact_owner_count = 0
    menu_exact_item_count = 0
    menu_owner_check_code = 0
}

function Write-TrayFailure([string] $Code) {
    [Console]::Out.WriteLine($Code)
    [Console]::Out.WriteLine("tray_diagnostics=" + ($script:TrayDiagnostics | ConvertTo-Json -Compress))
}

try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type -AssemblyName System.Windows.Forms

    Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class ChadexTrayInput {
    [StructLayout(LayoutKind.Sequential)]
    public struct TrayRect {
        public int left, top, right, bottom;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct NotifyIconIdentifier {
        public uint cbSize;
        public IntPtr hWnd;
        public uint uID;
        public Guid guidItem;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct Point {
        public int x, y;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct MouseInput {
        public int dx, dy;
        public uint mouseData, flags, time;
        public UIntPtr extraInfo;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeInput {
        public uint type;
        public MouseInput mouse;
    }
    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")]
    private static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")]
    private static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out Point point);
    [DllImport("shell32.dll", PreserveSig = true)]
    private static extern int Shell_NotifyIconGetRect(
        ref NotifyIconIdentifier identifier, out TrayRect iconLocation);
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
    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint SendInput(uint inputCount, [In] NativeInput[] inputs, int inputSize);
    public static uint SendMouseInput(uint flags) {
        var input = new NativeInput {
            type = 0, // INPUT_MOUSE
            mouse = new MouseInput { flags = flags, extraInfo = UIntPtr.Zero }
        };
        return SendInput(1, new[] { input }, Marshal.SizeOf(typeof(NativeInput)));
    }
    public static bool CursorMatches(int x, int y) {
        Point point;
        return GetCursorPos(out point) && point.x == x && point.y == y;
    }
    public static int ShellTrayRootKind(int x, int y, uint expectedShellProcessId) {
        var hwnd = WindowFromPoint(new Point { x = x, y = y });
        if (WindowProcessId(hwnd) != expectedShellProcessId) return 0;
        var root = GetAncestor(hwnd, 2); // GA_ROOT
        if (root == IntPtr.Zero || WindowProcessId(root) != expectedShellProcessId) return 0;
        var className = new StringBuilder(256);
        if (GetClassName(root, className, className.Capacity) == 0) return 0;
        // Sanitized diagnostics: 1=taskbar, 2=legacy overflow, 3=Win11 overflow.
        switch (className.ToString()) {
            case "Shell_TrayWnd": return 1;
            case "NotifyIconOverflowWindow": return 2;
            case "TopLevelWindowForOverflowXamlIsland": return 3;
            default: return 0;
        }
    }
    public static uint WindowProcessId(IntPtr hwnd) {
        uint processId = 0;
        return hwnd != IntPtr.Zero && GetWindowThreadProcessId(hwnd, out processId) != 0 ? processId : 0;
    }
    public static IntPtr[] FindTrayIconWindows(uint expectedProcessId) {
        var handles = new List<IntPtr>();
        var completed = EnumWindows((hwnd, _) => {
            if (WindowProcessId(hwnd) != expectedProcessId) return true;
            var className = new StringBuilder(256);
            if (GetClassName(hwnd, className, className.Capacity) != 0 &&
                className.ToString() == "tray_icon_app") handles.Add(hwnd);
            return true;
        }, IntPtr.Zero);
        return completed ? handles.ToArray() : Array.Empty<IntPtr>();
    }
    public static bool TryGetTrayIconRect(IntPtr hwnd, uint iconId, out TrayRect rect) {
        var identifier = new NotifyIconIdentifier {
            cbSize = (uint)Marshal.SizeOf(typeof(NotifyIconIdentifier)),
            hWnd = hwnd,
            uID = iconId,
            guidItem = Guid.Empty
        };
        return Shell_NotifyIconGetRect(ref identifier, out rect) == 0;
    }
    public static int MenuOwnerCheckCode(int nativeHandle, uint processId, IntPtr expectedOwnerHwnd) {
        var hwnd = new IntPtr(unchecked((long)(uint)nativeHandle));
        // Sanitized diagnostics: 1=missing hwnd/thread, 2=other process,
        // 3=no active menu, 4=wrong owner PID, 5=wrong HWND, 6=wrong class, 7=exact app.
        if (hwnd == IntPtr.Zero) return 1; // no native menu window
        uint menuProcessId;
        uint threadId = GetWindowThreadProcessId(hwnd, out menuProcessId);
        if (threadId == 0) return 1;
        if (menuProcessId != processId) return 2; // menu belongs to another process
        var info = new GuiThreadInfo();
        info.cbSize = (uint)Marshal.SizeOf(typeof(GuiThreadInfo));
        if (!GetGUIThreadInfo(threadId, ref info) || (info.flags & 0x14) == 0)
            return 3; // no active popup/menu state
        if (WindowProcessId(info.hwndMenuOwner) != processId) return 4;
        if (info.hwndMenuOwner != expectedOwnerHwnd) return 5;
        var ownerClass = new StringBuilder(256);
        if (GetClassName(info.hwndMenuOwner, ownerClass, ownerClass.Capacity) == 0 ||
            ownerClass.ToString() != "tray_icon_app") return 6;
        return 7; // exact Chadex tray window owns the active menu
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

    $trayWindows = @([ChadexTrayInput]::FindTrayIconWindows([uint32]$AppProcessId))
    $script:TrayDiagnostics.native_window_count = $trayWindows.Count
    if ($trayWindows.Count -ne 1) {
        Write-TrayFailure "tray_icon_unavailable"
        exit 2
    }
    $script:ExpectedTrayWindow = $trayWindows[0]

    # main.rs enables single-instance, but that is NOT OS-level ownership proof.
    # The pinned tray-icon 0.25.1 passes its tray_icon_app HWND to TrackPopupMenu;
    # menu discovery checks the exact active owner, never a guessed HWND.
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
    $script:TrayDiagnostics.taskbar_found = $true

    $shellProcessId = $taskbar.Current.ProcessId
    $shellProcess = Get-Process -Id $shellProcessId -ErrorAction SilentlyContinue
    $script:TrayDiagnostics.same_session_as_shell = (
        $null -ne $shellProcess -and
        $shellProcess.SessionId -eq [System.Diagnostics.Process]::GetCurrentProcess().SessionId
    )
    function Get-NotificationScopes {
        # Never search all Shell_TrayWnd for Chadex: it also contains app buttons.
        $found = @()
        foreach ($className in @("TrayNotifyWnd", "SystemTray.SystemTrayFrame")) {
            $areas = $taskbar.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.PropertyCondition]::new(
                    [System.Windows.Automation.AutomationElement]::ClassNameProperty, $className)
            )
            foreach ($area in $areas) {
                if ($area.Current.ProcessId -eq $shellProcessId) { $found += $area }
            }
        }
        $script:TrayDiagnostics.notification_scope_count = $found.Count
        return $found
    }
    function Get-TrayScopes {
        $notificationScopes = @(Get-NotificationScopes)
        $overflowScopes = @()
        foreach ($className in @("NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland")) {
            $overflows = $desktop.FindAll(
                [System.Windows.Automation.TreeScope]::Children,
                [System.Windows.Automation.PropertyCondition]::new(
                    [System.Windows.Automation.AutomationElement]::ClassNameProperty, $className)
            )
            foreach ($overflow in $overflows) {
                if ($overflow.Current.ProcessId -eq $shellProcessId) { $overflowScopes += $overflow }
            }
        }
        $script:TrayDiagnostics.overflow_scope_count = $overflowScopes.Count
        return @($notificationScopes) + @($overflowScopes)
    }
    function Find-ChadexTrayIcon {
        $candidates = @{}
        $metrics = @{
            name_match_count = 0
            shell_process_match_count = 0
            visible_match_count = 0
            enabled_match_count = 0
            supported_control_match_count = 0
        }
        foreach ($scope in @(Get-TrayScopes)) {
            $elements = $scope.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition
            )
            foreach ($element in $elements) {
                try {
                    $current = $element.Current
                    if ($current.Name -notmatch '^Chadex(?:\s|$)') { continue }
                    $metrics.name_match_count++
                    if ($current.ProcessId -ne $shellProcessId) { continue }
                    $metrics.shell_process_match_count++
                    if ($current.IsOffscreen) { continue }
                    $metrics.visible_match_count++
                    if (-not $current.IsEnabled) { continue }
                    $metrics.enabled_match_count++
                    if ($current.ControlType -ne [System.Windows.Automation.ControlType]::Button -and
                        $current.ControlType -ne [System.Windows.Automation.ControlType]::Custom) { continue }
                    $metrics.supported_control_match_count++
                    $candidates[($element.GetRuntimeId() -join ":")] = $element
                } catch [System.Windows.Automation.ElementNotAvailableException] {
                    # Explorer can rebuild its notification elements while polling.
                }
            }
        }
        foreach ($name in $metrics.Keys) { $script:TrayDiagnostics[$name] = $metrics[$name] }
        $script:TrayDiagnostics.candidate_count = $candidates.Count
        if ($candidates.Count -gt 1) { throw "tray_icon_ambiguous" }
        if ($candidates.Count -eq 1) { return @($candidates.Values)[0] }
        return $null
    }
    function Find-NativeTrayIcon {
        if (-not $script:TrayDiagnostics.same_session_as_shell) { return $null }
        $windows = @([ChadexTrayInput]::FindTrayIconWindows([uint32]$AppProcessId))
        $script:TrayDiagnostics.native_window_count = $windows.Count
        $nativeMatches = @()
        foreach ($window in $windows) {
            # tray-icon 0.25.1 allocates notification IDs from 1; bound discovery
            # and require exactly one Shell-owned icon for this exact app window.
            for ($iconId = 1; $iconId -le 16; $iconId++) {
                $rect = [ChadexTrayInput+TrayRect]::new()
                if ([ChadexTrayInput]::TryGetTrayIconRect(
                    $window, [uint32]$iconId, [ref]$rect
                ) -and $rect.right -gt $rect.left -and $rect.bottom -gt $rect.top) {
                    $nativeMatches += [PSCustomObject]@{
                        Window = $window
                        IconId = $iconId
                        Rect = $rect
                    }
                }
            }
        }
        $script:TrayDiagnostics.native_icon_match_count = $nativeMatches.Count
        if ($nativeMatches.Count -gt 1) { throw "tray_icon_ambiguous" }
        if ($nativeMatches.Count -eq 1) {
            $script:TrayDiagnostics.native_rect_available = $true
            return $nativeMatches[0]
        }
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
        if (-not (Test-AppIdentity) -or $menu.Current.ProcessId -ne $AppProcessId -or
            $menu.Current.IsOffscreen) { return $false }
        $script:TrayDiagnostics.menu_owner_check_code = [ChadexTrayInput]::MenuOwnerCheckCode(
            $menu.Current.NativeWindowHandle, [uint32]$AppProcessId,
            $script:ExpectedTrayWindow)
        return $script:TrayDiagnostics.menu_owner_check_code -eq 7
    }
    function Find-ChadexMenu {
        $elements = $desktop.FindAll(
            [System.Windows.Automation.TreeScope]::Children,
            [System.Windows.Automation.PropertyCondition]::new(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::Menu)
        )
        $script:TrayDiagnostics.menu_element_count = [Math]::Max(
            $script:TrayDiagnostics.menu_element_count, $elements.Count)
        $menuCandidates = @()
        foreach ($element in $elements) {
            try {
                if (-not (Test-AppIdentity) -or $element.Current.IsOffscreen) { continue }
                if ($element.Current.ProcessId -eq $AppProcessId) {
                    $script:TrayDiagnostics.menu_app_process_count++
                }
                $ownerCode = [ChadexTrayInput]::MenuOwnerCheckCode(
                    $element.Current.NativeWindowHandle, [uint32]$AppProcessId,
                    $script:ExpectedTrayWindow)
                $script:TrayDiagnostics.menu_owner_check_code = $ownerCode
                if ($ownerCode -ne 7) { continue }
                $script:TrayDiagnostics.menu_exact_owner_count++
                $items = $element.FindAll(
                    [System.Windows.Automation.TreeScope]::Children,
                    [System.Windows.Automation.PropertyCondition]::new(
                        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                        [System.Windows.Automation.ControlType]::MenuItem)
                )
                $showItems = @($items | Where-Object { $_.Current.Name -ceq "Show Chadex" })
                $quitItems = @($items | Where-Object { $_.Current.Name -ceq "Quit" })
                if ($showItems.Count -eq 1 -and $quitItems.Count -eq 1) {
                    $script:TrayDiagnostics.menu_exact_item_count++
                }
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

    function Invoke-RightClickAt([int] $X, [int] $Y) {
        if (-not [ChadexTrayInput]::SetCursorPos($X, $Y)) { return $false }
        $script:TrayDiagnostics.native_cursor_matches_rect =
            [ChadexTrayInput]::CursorMatches($X, $Y)
        if (-not $script:TrayDiagnostics.native_cursor_matches_rect) { return $false }

        $script:TrayDiagnostics.native_click_attempted = $true
        $script:TrayDiagnostics.native_click_down_input_count =
            [int][ChadexTrayInput]::SendMouseInput(0x0008)
        Start-Sleep -Milliseconds 50
        $script:TrayDiagnostics.native_click_up_input_count =
            [int][ChadexTrayInput]::SendMouseInput(0x0010)
        if ($script:TrayDiagnostics.native_click_up_input_count -ne 1) {
            # One bounded release attempt prevents a failed down/up pair from leaving
            # a held right button. The operation still fails closed below.
            $script:TrayDiagnostics.native_click_up_input_count +=
                [int][ChadexTrayInput]::SendMouseInput(0x0010)
        }
        return ($script:TrayDiagnostics.native_click_down_input_count -eq 1 -and
            $script:TrayDiagnostics.native_click_up_input_count -eq 1)
    }

    $null = Find-ChadexMenu
    if ($script:TrayDiagnostics.menu_exact_owner_count -gt 0) {
        $script:TrayDiagnostics.preexisting_menu_count = 1
        Write-TrayFailure "tray_menu_unavailable"
        exit 3
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
                $script:TrayDiagnostics.overflow_toggle_count = $toggles.Count
                if ($toggles.Count -eq 1) {
                    # Do not toggle again if Explorer invalidates the element on invoke.
                    $overflowOpened = $true
                    $script:TrayDiagnostics.overflow_open_attempted = $true
                    Invoke-Element (@($toggles.Values)[0])
                }
            }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # Re-enumerate notification/overflow roots until the same deadline.
        }
        Start-Sleep -Milliseconds 100
    } while ($discovery.ElapsedMilliseconds -lt 4000)
    $selection = $null
    if ($null -eq $icon) {
        # Some Windows taskbar builds do not expose notification icons through
        # UIA. Resolve the Shell rectangle from the exact app-owned tray HWND/ID.
        $nativeIcon = Find-NativeTrayIcon
        if ($null -eq $nativeIcon -or -not (Test-AppIdentity) -or
            [ChadexTrayInput]::WindowProcessId($nativeIcon.Window) -ne $AppProcessId -or
            $nativeIcon.Window -ne $script:ExpectedTrayWindow) {
            Write-TrayFailure "tray_icon_unavailable"
            exit 2
        }
        $bounds = $nativeIcon.Rect
        $width = $bounds.right - $bounds.left
        $height = $bounds.bottom - $bounds.top
        if ($width -lt 2 -or $height -lt 2 -or $width -gt 128 -or $height -gt 128) {
            Write-TrayFailure "tray_icon_unavailable"
            exit 2
        }
        $x = [int]($bounds.left + ($width / 2))
        $y = [int]($bounds.top + ($height / 2))
        $script:TrayDiagnostics.native_rect_root_kind = [ChadexTrayInput]::ShellTrayRootKind(
            $x, $y, [uint32]$shellProcessId)
        if ($script:TrayDiagnostics.native_rect_root_kind -eq 0) {
            Write-TrayFailure "tray_icon_unavailable"
            exit 2
        }
        if (-not (Invoke-RightClickAt $x $y)) {
            Write-TrayFailure "tray_menu_unavailable"
            exit 3
        }
        $selection = Wait-ChadexMenu
        if ($null -eq $selection) {
            Write-TrayFailure "tray_menu_unavailable"
            exit 3
        }
    } else {
        # UIA name/scope is only a candidate, not AppProcessId proof. Opening it
        # must still be followed by the independent native menu-owner check.
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
                Write-TrayFailure "tray_menu_unavailable"
                exit 3
            }
            $bounds = $icon.Current.BoundingRectangle
            if ($bounds.IsEmpty -or $bounds.Width -lt 2 -or $bounds.Height -lt 2) {
                Write-TrayFailure "tray_menu_unavailable"
                exit 3
            }
            $x = [int]($bounds.Left + ($bounds.Width / 2))
            $y = [int]($bounds.Top + ($bounds.Height / 2))
            if (-not (Invoke-RightClickAt $x $y)) {
                Write-TrayFailure "tray_menu_unavailable"
                exit 3
            }
            $selection = Wait-ChadexMenu
            if ($null -eq $selection) {
                Write-TrayFailure "tray_menu_unavailable"
                exit 3
            }
        }
    }
    # Revalidate immediately before invoking; never trust menu labels alone.
    if (-not (Test-MenuOwner $selection.Menu) -or
        $selection.Quit.Current.ProcessId -ne $AppProcessId -or
        $selection.Quit.Current.Name -cne "Quit" -or
        -not $selection.Quit.Current.IsEnabled -or $selection.Quit.Current.IsOffscreen) {
        Write-TrayFailure "tray_menu_unavailable"
        exit 3
    }
    Invoke-Element $selection.Quit
    [Console]::Out.WriteLine("tray_quit_sent")
    exit 0
} catch {
    [Console]::Out.WriteLine("tray_quit_failed")
    exit 1
}
