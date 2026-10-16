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
    native_menu_state_code = 0
    post_input_menu_state_code = 0
    post_input_menu_mode_flags = 0
    post_input_menu_owner_matches_tray = $false
    post_callback_menu_state_code = 0
    post_callback_menu_mode_flags = 0
    post_callback_menu_owner_matches_tray = $false
    native_callback_post_attempted = $false
    native_callback_post_succeeded = $false
    native_callback_post_error_code = 0
    foreground_process_grant_attempted = $false
    foreground_process_grant_succeeded = $false
    foreground_process_grant_error_code = 0
    native_keyboard_fallback_attempted = $false
    native_keyboard_fallback_succeeded = $false
    native_keyboard_guard_code = 0
    native_keyboard_input_count = 0
    native_keyboard_input_error_code = 0
    post_keyboard_menu_state_code = 0
    preexisting_menu_count = 0
    menu_element_count = 0
    uia_menu_desktop_child_count = 0
    uia_menu_tray_owner_descendant_count = 0
    native_popup_window_count = 0
    uia_menu_popup_handle_root_count = 0
    uia_menu_popup_root_count = 0
    uia_menu_popup_descendant_item_count = 0
    msaa_menu_popup_count = 0
    msaa_menu_child_count = 0
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
    private struct KeyboardInput {
        public ushort virtualKey, scanCode;
        public uint flags, time;
        public UIntPtr extraInfo;
    }
    [StructLayout(LayoutKind.Explicit)]
    private struct InputUnion {
        [FieldOffset(0)] public MouseInput mouse;
        [FieldOffset(0)] public KeyboardInput keyboard;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeInput {
        public uint type;
        public InputUnion data;
    }
    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumThreadWindows(uint threadId, EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowRect(IntPtr hwnd, out TrayRect rect);
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
    public struct MenuState {
        public int state_code;
        public uint mode_flags;
        public int owner_matches_tray;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr hwnd, StringBuilder className, int maxCount);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();
    [DllImport("oleacc.dll", PreserveSig = true)]
    private static extern int AccessibleObjectFromWindow(
        IntPtr hwnd, uint objectId, ref Guid interfaceId,
        [MarshalAs(UnmanagedType.Interface)] out object accessibleObject);
    [DllImport("user32.dll")]
    private static extern bool GetGUIThreadInfo(uint threadId, ref GuiThreadInfo info);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint SendInput(uint inputCount, [In] NativeInput[] inputs, int inputSize);
    [DllImport("user32.dll", EntryPoint = "PostMessageW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostMessage(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AllowSetForegroundWindow(uint processId);
    public static bool AllowExactProcessForeground(uint processId, out int errorCode) {
        var succeeded = processId != 0 && AllowSetForegroundWindow(processId);
        errorCode = succeeded ? 0 : Marshal.GetLastWin32Error();
        return succeeded;
    }
    public static uint SendMouseInput(uint flags) {
        var input = new NativeInput {
            type = 0, // INPUT_MOUSE
            data = new InputUnion {
                mouse = new MouseInput { flags = flags, extraInfo = UIntPtr.Zero }
            }
        };
        return SendInput(1, new[] { input }, Marshal.SizeOf(typeof(NativeInput)));
    }
    public static bool SendMenuQuitKeys(
        uint expectedProcessId, IntPtr expectedOwnerHwnd, IntPtr expectedPopupHwnd,
        out int sentCount, out int errorCode, out int guardCode, out int menuStateCode) {
        sentCount = 0;
        errorCode = 0;
        menuStateCode = 0;
        // Sanitized diagnostics: 0=not evaluated; 1=owner identity; 2=popup owner;
        // 3=GUI info; 4=popup mode; 5=menu owner; 6=no foreground; 7=foreground PID;
        // 8=foreground thread; 9=foreground HWND; 10=menu dismissed; 11=partial/failed SendInput;
        // 12=menu still active after keys; 13=post-input menu state invalid.
        guardCode = 0;
        uint ownerProcessId;
        uint ownerThreadId = GetWindowThreadProcessId(expectedOwnerHwnd, out ownerProcessId);
        if (ownerThreadId == 0 || ownerProcessId != expectedProcessId) {
            guardCode = 1;
            return false;
        }
        if (MenuOwnerCheckCode(expectedPopupHwnd, expectedProcessId, expectedOwnerHwnd) != 7) {
            guardCode = 2;
            return false;
        }

        var menuInfo = new GuiThreadInfo();
        menuInfo.cbSize = (uint)Marshal.SizeOf(typeof(GuiThreadInfo));
        if (!GetGUIThreadInfo(ownerThreadId, ref menuInfo)) {
            guardCode = 3;
            return false;
        }
        if ((menuInfo.flags & 0x10) == 0) {
            guardCode = 4;
            return false;
        }
        if (menuInfo.hwndMenuOwner != expectedOwnerHwnd) {
            guardCode = 5;
            return false;
        }

        var foreground = GetForegroundWindow();
        if (foreground == IntPtr.Zero) {
            guardCode = 6;
            return false;
        }
        uint foregroundProcessId;
        uint foregroundThreadId = GetWindowThreadProcessId(foreground, out foregroundProcessId);
        if (foregroundProcessId != expectedProcessId) {
            guardCode = 7;
            return false;
        }
        if (foregroundThreadId != ownerThreadId) {
            guardCode = 8;
            return false;
        }
        if (foreground != expectedOwnerHwnd && foreground != expectedPopupHwnd) {
            guardCode = 9;
            return false;
        }

        var inputs = new[] {
            new NativeInput {
                type = 1, // INPUT_KEYBOARD
                data = new InputUnion {
                    keyboard = new KeyboardInput { virtualKey = 0x28, extraInfo = UIntPtr.Zero }
                }
            },
            new NativeInput {
                type = 1, // INPUT_KEYBOARD
                data = new InputUnion {
                    keyboard = new KeyboardInput {
                        virtualKey = 0x28, flags = 0x0002, extraInfo = UIntPtr.Zero
                    }
                }
            },
            new NativeInput {
                type = 1, // INPUT_KEYBOARD
                data = new InputUnion {
                    keyboard = new KeyboardInput { virtualKey = 0x28, extraInfo = UIntPtr.Zero }
                }
            },
            new NativeInput {
                type = 1, // INPUT_KEYBOARD
                data = new InputUnion {
                    keyboard = new KeyboardInput {
                        virtualKey = 0x28, flags = 0x0002, extraInfo = UIntPtr.Zero
                    }
                }
            },
            new NativeInput {
                type = 1, // INPUT_KEYBOARD
                data = new InputUnion {
                    keyboard = new KeyboardInput { virtualKey = 0x0D, extraInfo = UIntPtr.Zero }
                }
            },
            new NativeInput {
                type = 1, // INPUT_KEYBOARD
                data = new InputUnion {
                    keyboard = new KeyboardInput {
                        virtualKey = 0x0D, flags = 0x0002, extraInfo = UIntPtr.Zero
                    }
                }
            }
        };
        sentCount = (int)SendInput(6, inputs, Marshal.SizeOf(typeof(NativeInput)));
        if (sentCount != 6) {
            errorCode = Marshal.GetLastWin32Error();
            guardCode = 11;
            return false;
        }
        // End is not part of the documented standard menu keyboard interface.
        // With the source-pinned [Show Chadex, Quit] order, two Down presses
        // select Quit; Enter must dismiss the exact active menu before success.
        var menuWait = System.Diagnostics.Stopwatch.StartNew();
        do {
            menuStateCode = ActiveMenuStateCode(expectedProcessId, expectedOwnerHwnd);
            if (menuStateCode == 0) {
                guardCode = 10;
                return true;
            }
            if (menuStateCode == 1) {
                guardCode = 13;
                return false;
            }
            if (menuStateCode != 2 && menuStateCode != 3) {
                guardCode = 13;
                return false;
            }
            System.Threading.Thread.Sleep(25);
        } while (menuWait.ElapsedMilliseconds < 2000);
        guardCode = menuStateCode == 3 ? 12 : 13;
        return false;
    }
    public static bool PostTrayRightButtonUp(IntPtr hwnd, uint iconId, out int errorCode) {
        // Runner-only synthesized callback delivery, not physical mouse input;
        // scalar-only tray-icon 0.25.1 message payload, with no direct app call.
        var succeeded = PostMessage(hwnd, 6002, new UIntPtr(iconId), new IntPtr(0x0205));
        errorCode = succeeded ? 0 : Marshal.GetLastWin32Error();
        return succeeded;
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
    public static IntPtr[] FindActivePopupMenuWindows(uint expectedProcessId, IntPtr expectedOwnerHwnd) {
        uint ownerProcessId;
        uint ownerThreadId = GetWindowThreadProcessId(expectedOwnerHwnd, out ownerProcessId);
        if (ownerThreadId == 0 || ownerProcessId != expectedProcessId) return Array.Empty<IntPtr>();

        var handles = new List<IntPtr>();
        EnumWindowsProc callback = (hwnd, _) => {
            uint windowProcessId;
            uint windowThreadId = GetWindowThreadProcessId(hwnd, out windowProcessId);
            if (windowThreadId != ownerThreadId || windowProcessId != expectedProcessId) return true;

            var className = new StringBuilder(64);
            TrayRect rect;
            if (GetClassName(hwnd, className, className.Capacity) == 0 ||
                className.ToString() != "#32768" || !IsWindowVisible(hwnd) ||
                !GetWindowRect(hwnd, out rect) || rect.right <= rect.left || rect.bottom <= rect.top)
                return true;

            if (MenuOwnerCheckCode(hwnd, expectedProcessId, expectedOwnerHwnd) == 7)
                handles.Add(hwnd);
            return true;
        };
        var completed = EnumThreadWindows(ownerThreadId, callback, IntPtr.Zero);
        return completed ? handles.ToArray() : Array.Empty<IntPtr>();
    }
    public static object GetAccessiblePopupMenu(IntPtr hwnd) {
        if (hwnd == IntPtr.Zero) return null;
        // OBJID_CLIENT on the exact visible #32768 popup exposes the system
        // IAccessible menu tree even when the UIA provider exposes no children.
        var interfaceId = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
        object accessibleObject;
        var result = AccessibleObjectFromWindow(hwnd, 0xFFFFFFFCu, ref interfaceId,
            out accessibleObject);
        return result == 0 ? accessibleObject : null;
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
        return MenuOwnerCheckCode(hwnd, processId, expectedOwnerHwnd);
    }
    public static int MenuOwnerCheckCode(IntPtr hwnd, uint processId, IntPtr expectedOwnerHwnd) {
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
    public static int ActiveMenuStateCode(uint expectedProcessId, IntPtr expectedOwnerHwnd) {
        // 0=no active menu, 1=owner HWND/PID unavailable, 2=GUIThreadInfo failed,
        // 3=menu owned by the exact tray HWND, 4=same-process other owner, 5=other process.
        uint ownerProcessId;
        uint threadId = GetWindowThreadProcessId(expectedOwnerHwnd, out ownerProcessId);
        if (threadId == 0 || ownerProcessId != expectedProcessId) return 1;
        var info = new GuiThreadInfo();
        info.cbSize = (uint)Marshal.SizeOf(typeof(GuiThreadInfo));
        if (!GetGUIThreadInfo(threadId, ref info)) return 2;
        if ((info.flags & 0x1C) == 0) return 0; // GUI_INMENUMODE | GUI_SYSTEMMENUMODE | GUI_POPUPMENUMODE
        if (info.hwndMenuOwner == expectedOwnerHwnd) return 3;
        if (WindowProcessId(info.hwndMenuOwner) == expectedProcessId) return 4;
        return 5;
    }
    public static MenuState ReadMenuState(uint expectedProcessId, IntPtr expectedOwnerHwnd) {
        var result = new MenuState();
        uint ownerProcessId;
        uint threadId = GetWindowThreadProcessId(expectedOwnerHwnd, out ownerProcessId);
        if (threadId == 0 || ownerProcessId != expectedProcessId) {
            result.state_code = 1;
            return result;
        }
        var info = new GuiThreadInfo();
        info.cbSize = (uint)Marshal.SizeOf(typeof(GuiThreadInfo));
        if (!GetGUIThreadInfo(threadId, ref info)) {
            result.state_code = 2;
            return result;
        }
        result.mode_flags = info.flags & 0x1C;
        result.owner_matches_tray = info.hwndMenuOwner == expectedOwnerHwnd ? 1 : 0;
        if (result.mode_flags == 0) result.state_code = 0;
        else if (result.owner_matches_tray == 1) result.state_code = 3;
        else if (WindowProcessId(info.hwndMenuOwner) == expectedProcessId) result.state_code = 4;
        else result.state_code = 5;
        return result;
    }
}
"@

    # Pin the caller's exact process, never discover an app by name. Keep its handle
    # open so an exited process/PID reuse cannot be accepted during polling.
    $process = Get-Process -Id $AppProcessId -ErrorAction SilentlyContinue
    if ($AppProcessId -le 0 -or $null -eq $process) {
        Write-TrayFailure "tray_app_unavailable"
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
        Write-TrayFailure "tray_app_unavailable"
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
        Write-TrayFailure "tray_taskbar_unavailable"
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
    function Get-MsaaMenuCandidate($popupHandle, [switch] $InvokeQuit) {
        if ($InvokeQuit) {
            $script:TrayDiagnostics.menu_exact_item_count = 0
            $script:TrayDiagnostics.menu_owner_check_code = 0
        }
        if (-not (Test-AppIdentity)) {
            return $null
        }
        $ownerCode = [ChadexTrayInput]::MenuOwnerCheckCode(
            $popupHandle, [uint32]$AppProcessId, $script:ExpectedTrayWindow)
        $script:TrayDiagnostics.menu_owner_check_code = $ownerCode
        if ($ownerCode -ne 7) { return $null }
        $accessible = [ChadexTrayInput]::GetAccessiblePopupMenu($popupHandle)
        if ($null -eq $accessible) { return $null }
        try {
            $rootRole = [int]$accessible.get_accRole(0)
            if ($rootRole -ne 11) { return $null } # ROLE_SYSTEM_MENUPOPUP
            $childCount = [int]$accessible.accChildCount
            if ($childCount -lt 0 -or $childCount -gt 256) { return $null }
            $script:TrayDiagnostics.msaa_menu_child_count = [Math]::Max(
                $script:TrayDiagnostics.msaa_menu_child_count, $childCount)
            $showItems = @()
            $quitItems = @()
            for ($childId = 1; $childId -le $childCount; $childId++) {
                $name = [string]$accessible.get_accName($childId)
                $role = [int]$accessible.get_accRole($childId)
                if ($role -ne 12) { continue } # ROLE_SYSTEM_MENUITEM
                $state = [int]$accessible.get_accState($childId)
                $item = [PSCustomObject]@{
                    ChildId = $childId
                    Current = [PSCustomObject]@{
                        ProcessId = $AppProcessId
                        Name = $name
                        IsEnabled = (($state -band 0x00000001) -eq 0)
                        IsOffscreen = (($state -band 0x00018000) -ne 0)
                    }
                }
                if ($name -ceq "Show Chadex") { $showItems += $item }
                if ($name -ceq "Quit") { $quitItems += $item }
            }
            if ($showItems.Count -ne 1 -or $quitItems.Count -ne 1) { return $null }
            $script:TrayDiagnostics.menu_exact_item_count++
            if (-not $showItems[0].Current.IsEnabled -or $showItems[0].Current.IsOffscreen -or
                -not $quitItems[0].Current.IsEnabled -or $quitItems[0].Current.IsOffscreen) {
                return $null
            }
            if ($InvokeQuit) {
                # Revalidate the exact active native owner immediately before
                # invoking the accessible Quit menu item; never send a direct app command.
                if (-not (Test-AppIdentity) -or
                    [ChadexTrayInput]::MenuOwnerCheckCode(
                        $popupHandle, [uint32]$AppProcessId, $script:ExpectedTrayWindow) -ne 7) {
                    return $null
                }
                $accessible.accDoDefaultAction($quitItems[0].ChildId)
                return $true
            }
            return [PSCustomObject]@{
                Backend = "msaa"
                Menu = $null
                PopupHandle = $popupHandle
                Show = $showItems[0]
                Quit = $quitItems[0]
            }
        } catch {
            # An unavailable or malformed accessibility provider is not identity evidence.
            return $null
        } finally {
            if ([Runtime.InteropServices.Marshal]::IsComObject($accessible)) {
                [void][Runtime.InteropServices.Marshal]::ReleaseComObject($accessible)
            }
        }
    }
    function Find-ChadexMenu {
        # These counts describe one current tree query, not the number of polling
        # attempts; Wait-ChadexMenu retries while Explorer publishes the popup.
        $script:TrayDiagnostics.menu_exact_owner_count = 0
        $script:TrayDiagnostics.menu_exact_item_count = 0
        $script:TrayDiagnostics.menu_owner_check_code = 0
        $exactOwnerHandles = @{}
        $condition = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::Menu)
        $desktopElements = $desktop.FindAll(
            [System.Windows.Automation.TreeScope]::Children, $condition)
        $script:TrayDiagnostics.uia_menu_desktop_child_count = [Math]::Max(
            $script:TrayDiagnostics.uia_menu_desktop_child_count, $desktopElements.Count)
        # Native popup menus may be absent from both the desktop and owner UIA trees.
        # Also inspect UIA roots obtained from visible #32768 windows on the exact
        # owner thread; the native owner/PID and exact menu items remain mandatory.
        $ownerElements = @()
        try {
            $owner = [System.Windows.Automation.AutomationElement]::FromHandle(
                $script:ExpectedTrayWindow)
            if ($null -ne $owner) {
                $ownerElements = $owner.FindAll(
                    [System.Windows.Automation.TreeScope]::Descendants, $condition)
            }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # The exact owner may be rebuilding its UIA provider; retry next poll.
        }
        $script:TrayDiagnostics.uia_menu_tray_owner_descendant_count = [Math]::Max(
            $script:TrayDiagnostics.uia_menu_tray_owner_descendant_count, $ownerElements.Count)
        $popupHandles = @([ChadexTrayInput]::FindActivePopupMenuWindows(
            [uint32]$AppProcessId, $script:ExpectedTrayWindow))
        $script:TrayDiagnostics.native_popup_window_count = [Math]::Max(
            $script:TrayDiagnostics.native_popup_window_count, $popupHandles.Count)
        $popupRoots = @()
        $popupRootEntries = @()
        $popupHandleRootCount = 0
        foreach ($popupHandle in $popupHandles) {
            try {
                $popupRoot = [System.Windows.Automation.AutomationElement]::FromHandle($popupHandle)
                if ($null -eq $popupRoot) {
                    # Keep the exact native popup HWND for the MSAA fallback even
                    # when the UIA provider cannot create a root for it.
                    $popupRootEntries += [PSCustomObject]@{
                        Handle = $popupHandle
                        Root = $null
                    }
                    continue
                }
                $popupHandleRootCount++
                $popupRootEntries += [PSCustomObject]@{
                    Handle = $popupHandle
                    Root = $popupRoot
                }
                if ($popupRoot.Current.ControlType -eq [System.Windows.Automation.ControlType]::Menu) {
                    $popupRoots += $popupRoot
                }
            } catch [System.Windows.Automation.ElementNotAvailableException] {
                # Preserve the HWND for MSAA; the exact owner check below rejects
                # it if the native menu closed during UIA lookup.
                $popupRootEntries += [PSCustomObject]@{
                    Handle = $popupHandle
                    Root = $null
                }
            }
        }
        $script:TrayDiagnostics.uia_menu_popup_handle_root_count = [Math]::Max(
            $script:TrayDiagnostics.uia_menu_popup_handle_root_count, $popupHandleRootCount)
        $script:TrayDiagnostics.uia_menu_popup_root_count = [Math]::Max(
            $script:TrayDiagnostics.uia_menu_popup_root_count, $popupRoots.Count)
        $uniqueElements = @{}
        foreach ($element in @($desktopElements) + @($ownerElements) + @($popupRoots)) {
            try { $uniqueElements[($element.GetRuntimeId() -join ":")] = $element }
            catch [System.Windows.Automation.ElementNotAvailableException] {
                # Re-query after Explorer or the native menu changes its UIA tree.
            }
        }
        $elements = @($uniqueElements.Values)
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
                $ownerHandle = [Int64]$element.Current.NativeWindowHandle
                if (-not $exactOwnerHandles.ContainsKey($ownerHandle)) {
                    $exactOwnerHandles[$ownerHandle] = $true
                    $script:TrayDiagnostics.menu_exact_owner_count++
                }
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
                    $showItems[0].Current.ProcessId -eq $AppProcessId -and
                    $showItems[0].Current.IsEnabled -and -not $showItems[0].Current.IsOffscreen -and
                    $quitItems[0].Current.ProcessId -eq $AppProcessId -and
                    $quitItems[0].Current.IsEnabled -and -not $quitItems[0].Current.IsOffscreen) {
                    $menuCandidates += [PSCustomObject]@{
                        Backend = "uia"
                        Menu = $element
                        PopupHandle = [IntPtr]::Zero
                        Show = [PSCustomObject]@{
                            Current = $showItems[0].Current
                            Element = $showItems[0]
                        }
                        Quit = [PSCustomObject]@{
                            Current = $quitItems[0].Current
                            Element = $quitItems[0]
                        }
                    }
                }
            } catch [System.Windows.Automation.ElementNotAvailableException] {
                # A closing/replaced menu is not identity evidence.
            }
        }
        # Some Windows UIA providers expose the native #32768 HWND as a Window
        # root rather than a Menu control. Search its descendants only after the
        # ordinary menu tree yielded no candidate; the exact native owner HWND,
        # app process, unique labels, visibility and enabled state remain required.
        if ($menuCandidates.Count -eq 0) {
            $popupItemCondition = [System.Windows.Automation.PropertyCondition]::new(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::MenuItem)
            foreach ($popupEntry in $popupRootEntries) {
                if (-not (Test-AppIdentity)) { continue }
                $ownerCode = [ChadexTrayInput]::MenuOwnerCheckCode(
                    $popupEntry.Handle, [uint32]$AppProcessId, $script:ExpectedTrayWindow)
                $script:TrayDiagnostics.menu_owner_check_code = $ownerCode
                if ($ownerCode -ne 7) { continue }
                $popupHandleKey = [Int64]$popupEntry.Handle.ToInt64()
                if (-not $exactOwnerHandles.ContainsKey($popupHandleKey)) {
                    $exactOwnerHandles[$popupHandleKey] = $true
                    $script:TrayDiagnostics.menu_exact_owner_count++
                }
                $popupItems = @()
                if ($null -ne $popupEntry.Root) {
                    try {
                        $popupItems = $popupEntry.Root.FindAll(
                            [System.Windows.Automation.TreeScope]::Descendants,
                            $popupItemCondition)
                    } catch [System.Windows.Automation.ElementNotAvailableException] {
                        # The exact native owner remains eligible for MSAA below.
                    }
                }
                $uniquePopupItems = @{}
                foreach ($item in $popupItems) {
                    try { $uniquePopupItems[($item.GetRuntimeId() -join ":")] = $item }
                    catch [System.Windows.Automation.ElementNotAvailableException] {
                        # Re-query after the popup provider replaces a stale item.
                    }
                }
                $script:TrayDiagnostics.uia_menu_popup_descendant_item_count = [Math]::Max(
                    $script:TrayDiagnostics.uia_menu_popup_descendant_item_count,
                    $uniquePopupItems.Count)
                $showItems = @($uniquePopupItems.Values | Where-Object {
                    $_.Current.Name -ceq "Show Chadex"
                })
                $quitItems = @($uniquePopupItems.Values | Where-Object {
                    $_.Current.Name -ceq "Quit"
                })
                if ($showItems.Count -eq 1 -and $quitItems.Count -eq 1) {
                    $script:TrayDiagnostics.menu_exact_item_count++
                }
                if ($showItems.Count -eq 1 -and $quitItems.Count -eq 1 -and
                    $showItems[0].Current.ProcessId -eq $AppProcessId -and
                    $showItems[0].Current.IsEnabled -and -not $showItems[0].Current.IsOffscreen -and
                    $quitItems[0].Current.ProcessId -eq $AppProcessId -and
                    $quitItems[0].Current.IsEnabled -and -not $quitItems[0].Current.IsOffscreen) {
                    $menuCandidates += [PSCustomObject]@{
                        Backend = "uia"
                        Menu = $popupEntry.Root
                        PopupHandle = $popupEntry.Handle
                        Show = [PSCustomObject]@{
                            Current = $showItems[0].Current
                            Element = $showItems[0]
                        }
                        Quit = [PSCustomObject]@{
                            Current = $quitItems[0].Current
                            Element = $quitItems[0]
                        }
                    }
                } elseif ($popupItems.Count -eq 0) {
                    # Windows may expose #32768 as a UIA Window with no children.
                    # Use the native MSAA popup object, but keep exact owner, role,
                    # unique labels, enabled/visible checks, and accessible action.
                    $script:TrayDiagnostics.msaa_menu_popup_count = [Math]::Max(
                        $script:TrayDiagnostics.msaa_menu_popup_count, $popupHandles.Count)
                    $msaaCandidate = Get-MsaaMenuCandidate $popupEntry.Handle
                    if ($null -ne $msaaCandidate) { $menuCandidates += $msaaCandidate }
                }
            }
        }
        if ($menuCandidates.Count -gt 1 -or
            $script:TrayDiagnostics.menu_exact_owner_count -gt 1) {
            throw "tray_menu_ambiguous"
        }
        if ($menuCandidates.Count -eq 1) { return $menuCandidates[0] }
        return $null
    }

    function Record-NativeMenuState([string] $Phase) {
        $state = [ChadexTrayInput]::ReadMenuState(
            [uint32]$AppProcessId, $script:ExpectedTrayWindow)
        $script:TrayDiagnostics["${Phase}_menu_state_code"] = $state.state_code
        $script:TrayDiagnostics["${Phase}_menu_mode_flags"] = $state.mode_flags
        $script:TrayDiagnostics["${Phase}_menu_owner_matches_tray"] =
            ($state.owner_matches_tray -eq 1)
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

    function Invoke-NativeTrayCallbackFallback {
        # Use only after a real pointer-input attempt failed to expose a menu.
        # Re-resolve the exact Shell-owned (HWND, uID), then post tray-icon's
        # callback message; never invoke menu or quit code directly.
        if (-not (Test-AppIdentity)) { return $false }
        $nativeIcon = Find-NativeTrayIcon
        if ($null -eq $nativeIcon -or $nativeIcon.Window -ne $script:ExpectedTrayWindow -or
            [ChadexTrayInput]::WindowProcessId($nativeIcon.Window) -ne $AppProcessId) {
            return $false
        }
        $bounds = $nativeIcon.Rect
        $width = $bounds.right - $bounds.left
        $height = $bounds.bottom - $bounds.top
        if ($width -lt 2 -or $height -lt 2 -or $width -gt 128 -or $height -gt 128) {
            return $false
        }
        $x = [int]($bounds.left + ($width / 2))
        $y = [int]($bounds.top + ($height / 2))
        $script:TrayDiagnostics.native_rect_root_kind = [ChadexTrayInput]::ShellTrayRootKind(
            $x, $y, [uint32]$shellProcessId)
        if ($script:TrayDiagnostics.native_rect_root_kind -eq 0 -or
            -not [ChadexTrayInput]::SetCursorPos($x, $y) -or
            -not [ChadexTrayInput]::CursorMatches($x, $y)) {
            return $false
        }
        $script:TrayDiagnostics.native_cursor_matches_rect = $true

        # Do not post a second right-click if the first input opened a menu that
        # UI Automation could not discover. API failure or any menu state fails closed.
        $script:TrayDiagnostics.native_menu_state_code = [ChadexTrayInput]::ActiveMenuStateCode(
            [uint32]$AppProcessId, $script:ExpectedTrayWindow)
        if ($script:TrayDiagnostics.native_menu_state_code -ne 0) { return $false }

        # Revalidate the exact app/window after input and menu-state inspection.
        if (-not (Test-AppIdentity) -or
            [ChadexTrayInput]::WindowProcessId($nativeIcon.Window) -ne $AppProcessId) {
            return $false
        }

        # The app must be allowed to activate its own notification menu. Grant
        # foreground permission only to this verified app PID; never use ASFW_ANY.
        $foregroundGrantErrorCode = 0
        $script:TrayDiagnostics.foreground_process_grant_attempted = $true
        $script:TrayDiagnostics.foreground_process_grant_succeeded =
            [ChadexTrayInput]::AllowExactProcessForeground(
                [uint32]$AppProcessId, [ref]$foregroundGrantErrorCode)
        $script:TrayDiagnostics.foreground_process_grant_error_code = $foregroundGrantErrorCode
        if (-not $script:TrayDiagnostics.foreground_process_grant_succeeded) { return $false }

        $script:TrayDiagnostics.native_callback_post_attempted = $true
        $postErrorCode = 0
        $script:TrayDiagnostics.native_callback_post_succeeded =
            [ChadexTrayInput]::PostTrayRightButtonUp(
                $nativeIcon.Window, [uint32]$nativeIcon.IconId, [ref]$postErrorCode)
        $script:TrayDiagnostics.native_callback_post_error_code = $postErrorCode
        return $script:TrayDiagnostics.native_callback_post_succeeded
    }

    function Invoke-NativeMenuQuitFallback {
        # The release source contract pins the app menu to Show Chadex, then Quit.
        # Use real keyboard input on the verified foreground native menu; never
        # send an app command or call the app's quit handler directly.
        if (-not (Test-AppIdentity)) { return $false }
        $activePopupHandles = @([ChadexTrayInput]::FindActivePopupMenuWindows(
            [uint32]$AppProcessId, $script:ExpectedTrayWindow))
        if ($activePopupHandles.Count -ne 1) { return $false }
        $popupHandle = $activePopupHandles[0]
        $script:TrayDiagnostics.menu_exact_owner_count = $activePopupHandles.Count
        $script:TrayDiagnostics.menu_owner_check_code = [ChadexTrayInput]::MenuOwnerCheckCode(
            $popupHandle, [uint32]$AppProcessId, $script:ExpectedTrayWindow)
        if ($script:TrayDiagnostics.menu_owner_check_code -ne 7) {
            return $false
        }

        $script:TrayDiagnostics.native_keyboard_fallback_attempted = $true
        $sentCount = 0
        $errorCode = 0
        $guardCode = 0
        $menuStateCode = 0
        $script:TrayDiagnostics.native_keyboard_fallback_succeeded =
            [ChadexTrayInput]::SendMenuQuitKeys(
            [uint32]$AppProcessId, $script:ExpectedTrayWindow, $popupHandle,
            [ref]$sentCount, [ref]$errorCode, [ref]$guardCode, [ref]$menuStateCode)
        if (-not $script:TrayDiagnostics.native_keyboard_fallback_succeeded -and
            $guardCode -eq 13 -and $menuStateCode -eq 1) {
            # The exact app may destroy its tray HWND while processing Quit. Treat
            # that as success only when the retained exact-PID process handle exits.
            $exitWait = [System.Diagnostics.Stopwatch]::StartNew()
            do {
                $process.Refresh()
                if ($process.HasExited) {
                    $guardCode = 14
                    $script:TrayDiagnostics.native_keyboard_fallback_succeeded = $true
                    break
                }
                Start-Sleep -Milliseconds 25
            } while ($exitWait.ElapsedMilliseconds -lt 2000)
        }
        $script:TrayDiagnostics.native_keyboard_guard_code = $guardCode
        $script:TrayDiagnostics.native_keyboard_input_count = $sentCount
        $script:TrayDiagnostics.native_keyboard_input_error_code = $errorCode
        $script:TrayDiagnostics.post_keyboard_menu_state_code = $menuStateCode
        return $script:TrayDiagnostics.native_keyboard_fallback_succeeded
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
            Record-NativeMenuState "post_input"
            if (-not (Invoke-NativeTrayCallbackFallback)) {
                Write-TrayFailure "tray_menu_unavailable"
                exit 3
            }
            $selection = Wait-ChadexMenu
            if ($null -eq $selection) {
                Record-NativeMenuState "post_callback"
                if (-not (Invoke-NativeMenuQuitFallback)) {
                    Write-TrayFailure "tray_menu_unavailable"
                    exit 3
                }
                [Console]::Out.WriteLine("tray_quit_sent")
                [Console]::Out.WriteLine("tray_diagnostics=" + ($script:TrayDiagnostics | ConvertTo-Json -Compress))
                exit 0
            }
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
                Record-NativeMenuState "post_input"
                if (-not (Invoke-NativeTrayCallbackFallback)) {
                    Write-TrayFailure "tray_menu_unavailable"
                    exit 3
                }
                $selection = Wait-ChadexMenu
                if ($null -eq $selection) {
                    Record-NativeMenuState "post_callback"
                    if (-not (Invoke-NativeMenuQuitFallback)) {
                        Write-TrayFailure "tray_menu_unavailable"
                        exit 3
                    }
                    [Console]::Out.WriteLine("tray_quit_sent")
                    [Console]::Out.WriteLine("tray_diagnostics=" + ($script:TrayDiagnostics | ConvertTo-Json -Compress))
                    exit 0
                }
            }
        }
    }
    # Revalidate immediately before invoking; never trust menu labels alone.
    $activePopupHandles = @([ChadexTrayInput]::FindActivePopupMenuWindows(
        [uint32]$AppProcessId, $script:ExpectedTrayWindow))
    $script:TrayDiagnostics.menu_exact_owner_count = $activePopupHandles.Count
    $selectionOwnerValid = $activePopupHandles.Count -eq 1
    if ($selection.PopupHandle -ne [IntPtr]::Zero) {
        $selectionOwnerValid = $selectionOwnerValid -and
            ($activePopupHandles -contains $selection.PopupHandle)
        $script:TrayDiagnostics.menu_owner_check_code = [ChadexTrayInput]::MenuOwnerCheckCode(
            $selection.PopupHandle, [uint32]$AppProcessId, $script:ExpectedTrayWindow)
        $selectionOwnerValid = $selectionOwnerValid -and
            $script:TrayDiagnostics.menu_owner_check_code -eq 7
    } else {
        $selectionOwnerValid = $selectionOwnerValid -and (Test-MenuOwner $selection.Menu)
    }
    if (-not $selectionOwnerValid) {
        Write-TrayFailure "tray_menu_unavailable"
        exit 3
    }
    if ($selection.Backend -eq "msaa") {
        $script:TrayDiagnostics.msaa_menu_popup_count = $activePopupHandles.Count
        if ($null -eq (Get-MsaaMenuCandidate $selection.PopupHandle -InvokeQuit)) {
            Write-TrayFailure "tray_menu_unavailable"
            exit 3
        }
    } else {
        try {
            # UIA Current is a live query. The saved Current values in the
            # selection object are discovery snapshots and cannot authorize action.
            $showCurrent = $selection.Show.Element.Current
            $quitCurrent = $selection.Quit.Element.Current
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            Write-TrayFailure "tray_menu_unavailable"
            exit 3
        }
        if ($showCurrent.ProcessId -ne $AppProcessId -or
            $showCurrent.Name -cne "Show Chadex" -or
            -not $showCurrent.IsEnabled -or $showCurrent.IsOffscreen -or
            $quitCurrent.ProcessId -ne $AppProcessId -or
            $quitCurrent.Name -cne "Quit" -or
            -not $quitCurrent.IsEnabled -or $quitCurrent.IsOffscreen) {
            Write-TrayFailure "tray_menu_unavailable"
            exit 3
        }
        Invoke-Element $selection.Quit.Element
    }
    [Console]::Out.WriteLine("tray_quit_sent")
    [Console]::Out.WriteLine("tray_diagnostics=" + ($script:TrayDiagnostics | ConvertTo-Json -Compress))
    exit 0
} catch {
    Write-TrayFailure "tray_quit_failed"
    exit 1
}
