// Shared helpers for the platform probe scripts (loaded with Add-Type by common.ps1).
//
// - Win32 interop for window geometry, styles, foreground/focus, input injection and idle time.
// - TargetForm: a WinForms window (TextBox + MenuStrip) that plays the foreground app the panel must
//   never take focus from. It records activation, focus, menu and key events.
// - Expected panel geometry, computed independently of the app: content top-left at the cursor,
//   pushed back into the cursor monitor's work area; size 360x600 logical x DPI x text scale.
//
// Safety rules (enforced here, not only in the scripts): keys are injected only while our own form
// or the panel is the foreground window, and the mouse is pressed only over the panel or our form.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

public class TargetForm : Form {
    public TextBox Box;
    public MenuStrip Strip;
    public List<string> Log = new List<string>();
    public int MenuActivations;
    public int F9KeyDowns;
    int drained;

    public TargetForm() {
        Text = "kwikpaste-probe-target";
        StartPosition = FormStartPosition.Manual;
        KeyPreview = true;
        Strip = new MenuStrip();
        var file = new ToolStripMenuItem("File");
        file.DropDownItems.Add("Dummy");
        Strip.Items.Add(file);
        Controls.Add(Strip);
        MainMenuStrip = Strip;
        Box = new TextBox();
        Box.Left = 20;
        Box.Top = 60;
        Box.Width = 400;
        Box.Text = "probe";
        Controls.Add(Box);
        Activated += delegate { Add("form Activated"); };
        Deactivate += delegate { Add("form Deactivate"); };
        Box.GotFocus += delegate { Add("box GotFocus"); };
        Box.LostFocus += delegate { Add("box LostFocus"); };
        Strip.MenuActivate += delegate { MenuActivations++; Add("MENU ACTIVATED"); };
        KeyDown += delegate(object s, KeyEventArgs e) {
            if (e.KeyCode == Keys.F9) F9KeyDowns++;
            Add("KeyDown " + e.KeyCode);
        };
    }

    protected override void WndProc(ref Message m) {
        if (m.Msg == 0x0112) Add("WM_SYSCOMMAND 0x" + ((long)m.WParam & 0xFFF0).ToString("X"));
        base.WndProc(ref m);
    }

    public void Add(string s) {
        Log.Add(((double)Stopwatch.GetTimestamp() * 1000 / Stopwatch.Frequency).ToString("0") + " " + s);
    }

    /// Events recorded since the previous call.
    public string Drain() {
        var sb = new StringBuilder();
        for (int i = drained; i < Log.Count; i++) sb.Append(Log[i]).Append(" | ");
        drained = Log.Count;
        return sb.ToString();
    }
}

public static class Probe {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
    [StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }

    [DllImport("user32.dll")] public static extern IntPtr SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetFocus();
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint type);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int index);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromPoint(POINT p, uint flags);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfoW(IntPtr monitor, ref MONITORINFO info);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint x, out uint y);
    [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO lii);
    [DllImport("kernel32.dll")] public static extern uint GetTickCount();
    [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool RegisterHotKey(IntPtr h, int id, uint mods, uint vk);
    [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr h, int id);

    public const ushort VK_SHIFT = 0x10, VK_CONTROL = 0x11, VK_MENU = 0x12, VK_F9 = 0x78;
    static int vx, vy, vw, vh;

    /// Per-monitor DPI awareness, so every coordinate here is physical like the app's.
    public static void Init() {
        SetProcessDpiAwarenessContext((IntPtr)(-4));
        vx = GetSystemMetrics(76); vy = GetSystemMetrics(77); vw = GetSystemMetrics(78); vh = GetSystemMetrics(79);
    }

    public static string VirtualScreen() { return String.Format("{0},{1} {2}x{3}", vx, vy, vw, vh); }

    public static void Pump(int ms) {
        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < ms) { Application.DoEvents(); Thread.Sleep(2); }
    }

    public static uint IdleMs() {
        var lii = new LASTINPUTINFO(); lii.cbSize = 8;
        GetLastInputInfo(ref lii);
        return GetTickCount() - lii.dwTime;
    }

    public static bool InputDesktopAvailable() {
        IntPtr d = OpenInputDesktop(0, false, 0x0100);
        if (d == IntPtr.Zero) return false;
        CloseDesktop(d);
        return true;
    }

    /// True when Ctrl+Alt+Shift+F9 can still be registered (nobody holds it).
    public static bool DevHotkeyFree() {
        uint mods = 0x1 | 0x2 | 0x4 | 0x4000;
        if (!RegisterHotKey(IntPtr.Zero, 0x5150, mods, VK_F9)) return false;
        UnregisterHotKey(IntPtr.Zero, 0x5150);
        return true;
    }

    // ---------------------------------------------------------------- input

    static void Key(ushort vk, bool up) {
        var input = new INPUT[1];
        input[0].type = 1;
        input[0].u.ki.wVk = vk;
        input[0].u.ki.wScan = (ushort)MapVirtualKey(vk, 0);
        input[0].u.ki.dwFlags = up ? 0x2u : 0u;
        SendInput(1, input, Marshal.SizeOf(typeof(INPUT)));
    }

    /// Presses Ctrl+Alt+Shift+F9 (Ctrl first, so Windows never sees a lone Alt) and returns the
    /// Stopwatch timestamp taken right before F9 goes down.
    public static long DevHotkey(TargetForm form, IntPtr panel) {
        RequireForeground(form, panel, "the hotkey");
        Key(VK_CONTROL, false); Pump(8);
        Key(VK_MENU, false); Pump(8);
        Key(VK_SHIFT, false); Pump(8);
        long injected = Stopwatch.GetTimestamp();
        Key(VK_F9, false); Pump(25);
        Key(VK_F9, true); Pump(8);
        Key(VK_SHIFT, true); Pump(8);
        Key(VK_MENU, true); Pump(8);
        Key(VK_CONTROL, true);
        return injected;
    }

    static void Mouse(uint flags, int x, int y) {
        var input = new INPUT[1];
        input[0].type = 0;
        input[0].u.mi.dwFlags = flags;
        if ((flags & 0x8000) != 0) {
            input[0].u.mi.dx = (int)(((long)(x - vx) * 65535) / (vw - 1));
            input[0].u.mi.dy = (int)(((long)(y - vy) * 65535) / (vh - 1));
        }
        SendInput(1, input, Marshal.SizeOf(typeof(INPUT)));
    }

    /// Moves the real cursor (absolute, virtual desktop) and nudges it onto the exact pixel.
    public static POINT MoveTo(int x, int y) {
        Mouse(0x0001 | 0x8000 | 0x4000, x, y);
        Pump(30);
        POINT p; GetCursorPos(out p);
        if (p.X != x || p.Y != y) { SetCursorPos(x, y); Pump(20); GetCursorPos(out p); }
        return p;
    }

    /// Left click at the current cursor position, without moving between down and up
    /// (a press followed by a move would start a drag).
    public static void Click(TargetForm form, IntPtr panel) {
        POINT p; GetCursorPos(out p);
        IntPtr root = RootAt(p.X, p.Y);
        if (root != panel && root != form.Handle) {
            throw new Exception(String.Format("ABORT: cursor {0},{1} is over 0x{2:X}, not the panel or the probe form", p.X, p.Y, root.ToInt64()));
        }
        Mouse(0x0002, 0, 0); Pump(60);
        Mouse(0x0004, 0, 0);
    }

    /// Makes our own form the foreground window by clicking its text box: SetForegroundWindow is
    /// refused while another process owns the foreground, a real click on our own window is not.
    public static bool ClickIntoForeground(TargetForm form) {
        var center = form.Box.PointToScreen(new Point(form.Box.Width / 2, form.Box.Height / 2));
        MoveTo(center.X, center.Y);
        Pump(100);
        if (RootAt(center.X, center.Y) != form.Handle) return false;
        Mouse(0x0002, 0, 0); Pump(60);
        Mouse(0x0004, 0, 0); Pump(300);
        return GetForegroundWindow() == form.Handle;
    }

    /// Presses the left button at the current cursor position (which must be over the panel),
    /// moves by (dx, dy) in steps and releases: a drag on the panel's own frame.
    public static void DragBy(IntPtr panel, int dx, int dy, int steps) {
        POINT p; GetCursorPos(out p);
        IntPtr root = RootAt(p.X, p.Y);
        if (root != panel) {
            throw new Exception(String.Format("ABORT: drag start {0},{1} is over 0x{2:X}, not the panel", p.X, p.Y, root.ToInt64()));
        }
        Mouse(0x0002, 0, 0); Pump(80);
        for (int i = 1; i <= steps; i++) {
            MoveTo(p.X + dx * i / steps, p.Y + dy * i / steps);
            Pump(30);
        }
        Pump(60);
        Mouse(0x0004, 0, 0);
    }

    public static void RequireForeground(TargetForm form, IntPtr panel, string what) {
        IntPtr fg = GetForegroundWindow();
        if (fg != form.Handle && fg != panel) {
            throw new Exception(String.Format("ABORT before {0}: foreground is 0x{1:X}, not the probe form (someone else took the foreground)", what, fg.ToInt64()));
        }
    }

    // ---------------------------------------------------------------- windows

    public static int[] WindowRect(IntPtr h) {
        RECT r; GetWindowRect(h, out r);
        return new int[] { r.L, r.T, r.R, r.B };
    }

    public static int[] ClientRectOnScreen(IntPtr h) {
        RECT r; GetClientRect(h, out r);
        var origin = new POINT();
        ClientToScreen(h, ref origin);
        return new int[] { origin.X, origin.Y, origin.X + r.R, origin.Y + r.B };
    }

    public static IntPtr RootAt(int x, int y) {
        var p = new POINT(); p.X = x; p.Y = y;
        return GetAncestor(WindowFromPoint(p), 2);
    }

    public static long Style(IntPtr h) { return GetWindowLongPtr(h, -16).ToInt64(); }
    public static long ExStyle(IntPtr h) { return GetWindowLongPtr(h, -20).ToInt64(); }

    /// Asks the panel's window procedure (cross-process) what it answers to a left click.
    public static long MouseActivateReply(IntPtr h) {
        long lparam = (0x0201L << 16) | 1;   // MAKELPARAM(HTCLIENT, WM_LBUTTONDOWN)
        return SendMessageW(h, 0x0021, h, (IntPtr)lparam).ToInt64();
    }

    // ---------------------------------------------------------------- expected geometry

    public static double TextScaleFactor() {
        using (var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Accessibility")) {
            object value = key == null ? null : key.GetValue("TextScaleFactor");
            if (value == null) return 1.0;
            return Math.Min(2.25, Math.Max(1.0, Convert.ToDouble(value) / 100.0));
        }
    }

    static int RoundAway(double v) { return (int)Math.Round(v, MidpointRounding.AwayFromZero); }

    /// The content rect the app must produce for a cursor position, computed from scratch.
    public static int[] ExpectedClient(int cursorX, int cursorY) {
        var cursor = new POINT(); cursor.X = cursorX; cursor.Y = cursorY;
        IntPtr monitor = MonitorFromPoint(cursor, 2);
        var info = new MONITORINFO(); info.cbSize = Marshal.SizeOf(typeof(MONITORINFO));
        GetMonitorInfoW(monitor, ref info);
        uint dpiX, dpiY; GetDpiForMonitor(monitor, 0, out dpiX, out dpiY);
        double tsf = TextScaleFactor();
        double scale = (double)dpiX / 96.0;
        RECT work = info.rcWork;
        int workW = Math.Max(1, work.R - work.L), workH = Math.Max(1, work.B - work.T);
        int w = Math.Min(Math.Max(1, RoundAway(360.0 * tsf * scale)), workW);
        int h = Math.Min(Math.Max(1, RoundAway(600.0 * tsf * scale)), workH);
        int x = Math.Max(Math.Min(cursorX, work.R - w), work.L);
        int y = Math.Max(Math.Min(cursorY, work.B - h), work.T);
        return new int[] { x, y, x + w, y + h };
    }

    public static string Rect(int[] r) { return "[" + String.Join(",", r) + "]"; }
    public static bool Same(int[] a, int[] b) {
        if (a == null || b == null || a.Length != b.Length) return false;
        for (int i = 0; i < a.Length; i++) if (a[i] != b[i]) return false;
        return true;
    }

    // ---------------------------------------------------------------- probe log

    static long offset;

    /// Lines appended to the app's probe log since the previous call. The app keeps the file open
    /// for writing, so it is read with FileShare.ReadWrite.
    public static string[] ReadNewLines(string path) {
        if (!File.Exists(path)) return new string[0];
        using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete)) {
            if (stream.Length <= offset) return new string[0];
            stream.Seek(offset, SeekOrigin.Begin);
            var bytes = new byte[stream.Length - offset];
            int read = stream.Read(bytes, 0, bytes.Length);
            int end = Array.LastIndexOf(bytes, (byte)'\n', read - 1);
            if (end < 0) return new string[0];
            offset += end + 1;
            return Encoding.UTF8.GetString(bytes, 0, end + 1).Split(new[] { '\n' }, StringSplitOptions.RemoveEmptyEntries);
        }
    }

    public static void ResetLog() { offset = 0; }

    // ---------------------------------------------------------------- threads

    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
    [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr thread, out IntPtr description);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);

    /// The thread's description (Rust thread names show up here), or "" when it has none.
    public static string ThreadName(uint tid) {
        IntPtr thread = OpenThread(0x0800 | 0x1000, false, tid);
        if (thread == IntPtr.Zero) return "";
        string name = "";
        IntPtr text;
        if (GetThreadDescription(thread, out text) >= 0 && text != IntPtr.Zero) {
            name = Marshal.PtrToStringUni(text);
            LocalFree(text);
        }
        CloseHandle(thread);
        return name;
    }
}
