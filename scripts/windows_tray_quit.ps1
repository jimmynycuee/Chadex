param(
    [Parameter(Mandatory = $true)]
    [int] $AppProcessId
)

$ErrorActionPreference = "Stop"

try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type -AssemblyName System.Windows.Forms

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
        [Console]::Out.WriteLine("tray_icon_unavailable")
        exit 2
    }

    function Find-ChadexTrayIcon($scope) {
        if ($null -eq $scope) { return $null }
        $elements = $scope.FindAll(
            [System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition
        )
        foreach ($element in $elements) {
            $current = $element.Current
            if ($current.Name -match '^Chadex(?:\b|\s|$)' -and
                ($current.ControlType -eq [System.Windows.Automation.ControlType]::Button -or
                 $current.ControlType -eq [System.Windows.Automation.ControlType]::Custom)) {
                return $element
            }
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

    function Find-ChadexMenu($root) {
        $elements = $root.FindAll(
            [System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition
        )
        foreach ($element in $elements) {
            if ($element.Current.ControlType -ne [System.Windows.Automation.ControlType]::Menu) {
                continue
            }
            $items = $element.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition
            )
            $hasShow = $false
            $hasQuit = $false
            foreach ($item in $items) {
                if ($item.Current.Name -ceq "Show Chadex") { $hasShow = $true }
                if ($item.Current.Name -ceq "Quit") { $hasQuit = $true }
            }
            if ($hasShow -and $hasQuit) { return $element }
        }
        return $null
    }

    $process = Get-Process -Id $AppProcessId -ErrorAction SilentlyContinue
    if ($null -eq $process) {
        [Console]::Out.WriteLine("tray_icon_unavailable")
        exit 2
    }

    $icon = Find-ChadexTrayIcon $taskbar
    if ($null -eq $icon) {
        $buttons = $taskbar.FindAll(
            [System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition
        )
        foreach ($button in $buttons) {
            if ($button.Current.Name -match '(?i)hidden icons?' -and
                $button.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button) {
                Invoke-Element $button
                Start-Sleep -Milliseconds 250
                $overflowCondition = [System.Windows.Automation.PropertyCondition]::new(
                    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
                    "NotifyIconOverflowWindow"
                )
                $overflow = $desktop.FindFirst(
                    [System.Windows.Automation.TreeScope]::Children,
                    $overflowCondition
                )
                $icon = Find-ChadexTrayIcon $overflow
                if ($null -eq $icon) { $icon = Find-ChadexTrayIcon $taskbar }
                break
            }
        }
    }
    if ($null -eq $icon) {
        [Console]::Out.WriteLine("tray_icon_unavailable")
        exit 2
    }

    try {
        $icon.SetFocus()
        [System.Windows.Forms.SendKeys]::SendWait("+{F10}")
    } catch {
        [Console]::Out.WriteLine("tray_menu_unavailable")
        exit 3
    }
    Start-Sleep -Milliseconds 250

    $menu = Find-ChadexMenu $desktop
    if ($null -eq $menu) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class ChadexTrayInput {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);
}
"@
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
        Start-Sleep -Milliseconds 250
        $menu = Find-ChadexMenu $desktop
        if ($null -eq $menu) {
            [Console]::Out.WriteLine("tray_menu_unavailable")
            exit 3
        }
    }
    $items = $menu.FindAll(
        [System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition
    )
    $quit = $null
    foreach ($item in $items) {
        if ($item.Current.Name -ceq "Quit") {
            $quit = $item
            break
        }
    }
    if ($null -eq $quit) {
        [Console]::Out.WriteLine("tray_menu_unavailable")
        exit 3
    }
    Invoke-Element $quit
    [Console]::Out.WriteLine("tray_quit_sent")
    exit 0
} catch {
    [Console]::Out.WriteLine("tray_quit_failed")
    exit 1
}
