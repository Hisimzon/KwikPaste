// Raw Win32 clipboard and audio-session helpers for the real-clipboard probes (loaded by common.ps1).
//
// Clip writes exactly the formats a test asks for (WinForms would add its own), can have another window
// take the clipboard right after a copy (the app's read then retries), and reads formats back. Sessions lists
// the audio sessions of the default render device by process id: evidence that the copy sound played.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class Clip {
    [DllImport("user32.dll", SetLastError = true)] static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll")] static extern bool CloseClipboard();
    [DllImport("user32.dll")] static extern bool EmptyClipboard();
    [DllImport("user32.dll", SetLastError = true)] static extern IntPtr SetClipboardData(uint format, IntPtr mem);
    [DllImport("user32.dll")] static extern IntPtr GetClipboardData(uint format);
    [DllImport("user32.dll")] static extern uint EnumClipboardFormats(uint format);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern uint RegisterClipboardFormatW(string name);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClipboardFormatNameW(uint format, StringBuilder name, int size);
    [DllImport("user32.dll")] static extern uint GetClipboardSequenceNumber();
    [DllImport("kernel32.dll")] static extern IntPtr GlobalAlloc(uint flags, UIntPtr bytes);
    [DllImport("kernel32.dll")] static extern IntPtr GlobalLock(IntPtr mem);
    [DllImport("kernel32.dll")] static extern bool GlobalUnlock(IntPtr mem);
    [DllImport("kernel32.dll")] static extern UIntPtr GlobalSize(IntPtr mem);

    public static uint Sequence() { return GetClipboardSequenceNumber(); }

    static void Open() {
        for (int i = 0; i < 100; i++) {
            if (OpenClipboard(IntPtr.Zero)) return;
            Thread.Sleep(10);
        }
        throw new InvalidOperationException("the clipboard stays busy");
    }

    static uint FormatId(string name) {
        if (name.StartsWith("#")) return UInt32.Parse(name.Substring(1));
        return RegisterClipboardFormatW(name);
    }

    static string FormatName(uint format) {
        var sb = new StringBuilder(256);
        if (GetClipboardFormatNameW(format, sb, 256) > 0) return sb.ToString();
        return "#" + format;
    }

    static IntPtr Global(byte[] bytes) {
        IntPtr mem = GlobalAlloc(0x0002, (UIntPtr)(uint)bytes.Length);
        IntPtr p = GlobalLock(mem);
        Marshal.Copy(bytes, 0, p, bytes.Length);
        GlobalUnlock(mem);
        return mem;
    }

    /// Replaces the clipboard content with these formats (names like "HTML Format", or "#13" for a
    /// standard format id).
    public static void Set(Dictionary<string, byte[]> formats) {
        Open();
        try {
            EmptyClipboard();
            foreach (var pair in formats) {
                if (SetClipboardData(FormatId(pair.Key), Global(pair.Value)) == IntPtr.Zero) {
                    throw new InvalidOperationException("SetClipboardData " + pair.Key + " failed: " + Marshal.GetLastWin32Error());
                }
            }
        } finally {
            CloseClipboard();
        }
    }

    /// Like Set, but a second thread is already spinning on OpenClipboard when the new content is
    /// closed, so it takes the clipboard before the app's watcher can read it and holds it `holdMs`:
    /// the app's first read attempts meet a busy clipboard.
    /// The grabber opens the clipboard for its own window: OpenClipboard(NULL) does not keep other
    /// processes out, a window does.
    public static void SetContended(Dictionary<string, byte[]> formats, int holdMs) {
        var grabbed = new ManualResetEvent(false);
        var go = new ManualResetEvent(false);
        var created = new ManualResetEvent(false);
        var grabber = new Thread(() => {
            var window = new System.Windows.Forms.NativeWindow();
            window.CreateHandle(new System.Windows.Forms.CreateParams());
            created.Set();
            go.WaitOne();
            var watch = System.Diagnostics.Stopwatch.StartNew();
            while (!OpenClipboard(window.Handle)) {
                if (watch.ElapsedMilliseconds > 2000) { grabbed.Set(); window.DestroyHandle(); return; }
            }
            grabbed.Set();
            Thread.Sleep(holdMs);
            CloseClipboard();
            window.DestroyHandle();
        });
        grabber.SetApartmentState(ApartmentState.STA);
        grabber.Priority = ThreadPriority.Highest;
        grabber.Start();
        created.WaitOne();
        Open();
        try {
            EmptyClipboard();
            foreach (var pair in formats) {
                if (SetClipboardData(FormatId(pair.Key), Global(pair.Value)) == IntPtr.Zero) {
                    throw new InvalidOperationException("SetClipboardData " + pair.Key + " failed: " + Marshal.GetLastWin32Error());
                }
            }
            go.Set();
            Thread.Sleep(20);
        } finally {
            CloseClipboard();
        }
        grabbed.WaitOne();
        grabber.Join();
    }

    public static void SetText(string text) {
        var formats = new Dictionary<string, byte[]>();
        formats["#13"] = Text(text);
        Set(formats);
    }

    public static byte[] Text(string text) { return Encoding.Unicode.GetBytes(text + "\0"); }

    public static byte[] Ascii(string text) { return Encoding.ASCII.GetBytes(text + "\0"); }

    /// CF_HTML: UTF-8 with the byte offsets in the header.
    public static byte[] Html(string fragment) {
        const string header = "Version:0.9\r\nStartHTML:{0:D10}\r\nEndHTML:{1:D10}\r\nStartFragment:{2:D10}\r\nEndFragment:{3:D10}\r\n";
        const string pre = "<html><body>\r\n<!--StartFragment-->";
        const string post = "<!--EndFragment-->\r\n</body></html>";
        int startHtml = String.Format(header, 0, 0, 0, 0).Length;
        int startFragment = startHtml + Encoding.UTF8.GetByteCount(pre);
        int endFragment = startFragment + Encoding.UTF8.GetByteCount(fragment);
        int endHtml = endFragment + Encoding.UTF8.GetByteCount(post);
        string all = String.Format(header, startHtml, endHtml, startFragment, endFragment) + pre + fragment + post;
        byte[] bytes = Encoding.UTF8.GetBytes(all);
        var terminated = new byte[bytes.Length + 1];
        Array.Copy(bytes, terminated, bytes.Length);
        return terminated;
    }

    /// CF_HDROP: DROPFILES (pFiles = 20, fWide = 1) and a double-null-terminated UTF-16 list.
    public static byte[] Files(string[] paths) {
        byte[] list = Encoding.Unicode.GetBytes(String.Join("\0", paths) + "\0\0");
        var bytes = new byte[20 + list.Length];
        BitConverter.GetBytes(20).CopyTo(bytes, 0);
        BitConverter.GetBytes(1).CopyTo(bytes, 16);
        list.CopyTo(bytes, 20);
        return bytes;
    }

    static Bitmap Pattern(int width, int height, Color left, Color right) {
        var bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        using (var graphics = Graphics.FromImage(bitmap)) {
            using (var brush = new SolidBrush(left)) graphics.FillRectangle(brush, 0, 0, width / 2, height);
            using (var brush = new SolidBrush(right)) graphics.FillRectangle(brush, width / 2, 0, width - width / 2, height);
        }
        return bitmap;
    }

    /// A 32-bit bottom-up CF_DIB of a two-color pattern (left half, right half).
    public static byte[] Dib(int width, int height, Color left, Color right) {
        using (var bitmap = Pattern(width, height, left, right)) {
            var bytes = new byte[40 + width * height * 4];
            BitConverter.GetBytes(40).CopyTo(bytes, 0);
            BitConverter.GetBytes(width).CopyTo(bytes, 4);
            BitConverter.GetBytes(height).CopyTo(bytes, 8);
            BitConverter.GetBytes((short)1).CopyTo(bytes, 12);
            BitConverter.GetBytes((short)32).CopyTo(bytes, 14);
            BitConverter.GetBytes(width * height * 4).CopyTo(bytes, 20);
            int offset = 40;
            for (int y = height - 1; y >= 0; y--) {
                for (int x = 0; x < width; x++) {
                    Color c = bitmap.GetPixel(x, y);
                    bytes[offset++] = c.B; bytes[offset++] = c.G; bytes[offset++] = c.R; bytes[offset++] = 255;
                }
            }
            return bytes;
        }
    }

    public static byte[] Png(int width, int height, Color left, Color right) {
        using (var bitmap = Pattern(width, height, left, right))
        using (var stream = new MemoryStream()) {
            bitmap.Save(stream, ImageFormat.Png);
            return stream.ToArray();
        }
    }

    public static string[] Formats() {
        Open();
        try {
            var names = new List<string>();
            uint format = 0;
            while ((format = EnumClipboardFormats(format)) != 0) names.Add(FormatName(format));
            return names.ToArray();
        } finally {
            CloseClipboard();
        }
    }

    public static byte[] Get(string name) {
        Open();
        try {
            IntPtr handle = GetClipboardData(FormatId(name));
            if (handle == IntPtr.Zero) return null;
            int size = (int)(uint)GlobalSize(handle);
            IntPtr p = GlobalLock(handle);
            var bytes = new byte[size];
            Marshal.Copy(p, bytes, 0, size);
            GlobalUnlock(handle);
            return bytes;
        } finally {
            CloseClipboard();
        }
    }

    public static string GetText() {
        byte[] bytes = Get("#13");
        if (bytes == null) return null;
        string text = Encoding.Unicode.GetString(bytes);
        int end = text.IndexOf('\0');
        return end >= 0 ? text.Substring(0, end) : text;
    }

    /// The CF_HDROP file list on the clipboard, or null.
    public static string[] GetFiles() {
        byte[] bytes = Get("#15");
        if (bytes == null || bytes.Length < 20) return null;
        int offset = BitConverter.ToInt32(bytes, 0);
        string list = Encoding.Unicode.GetString(bytes, offset, bytes.Length - offset);
        return list.Split(new[] { '\0' }, StringSplitOptions.RemoveEmptyEntries);
    }
}

[ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDeviceEnumerator {
    [PreserveSig] int EnumAudioEndpoints(int dataFlow, int stateMask, out IntPtr devices);
    [PreserveSig] int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice device);
}

[ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDevice {
    [PreserveSig] int Activate(ref Guid iid, int clsCtx, IntPtr activationParams, [MarshalAs(UnmanagedType.IUnknown)] out object instance);
}

[ComImport, Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionManager2 {
    [PreserveSig] int GetAudioSessionControl(IntPtr sessionGuid, int flags, out IntPtr control);
    [PreserveSig] int GetSimpleAudioVolume(IntPtr sessionGuid, int flags, out IntPtr volume);
    [PreserveSig] int GetSessionEnumerator(out IAudioSessionEnumerator sessions);
}

[ComImport, Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionEnumerator {
    [PreserveSig] int GetCount(out int count);
    [PreserveSig] int GetSession(int index, out IAudioSessionControl2 session);
}

[ComImport, Guid("bfb7ff88-7239-4fc9-8fa2-07c950be9c6d"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionControl2 {
    [PreserveSig] int GetState(out int state);
    [PreserveSig] int GetDisplayName(out IntPtr name);
    [PreserveSig] int SetDisplayName(string name, IntPtr context);
    [PreserveSig] int GetIconPath(out IntPtr path);
    [PreserveSig] int SetIconPath(string path, IntPtr context);
    [PreserveSig] int GetGroupingParam(out Guid grouping);
    [PreserveSig] int SetGroupingParam(ref Guid grouping, IntPtr context);
    [PreserveSig] int RegisterAudioSessionNotification(IntPtr client);
    [PreserveSig] int UnregisterAudioSessionNotification(IntPtr client);
    [PreserveSig] int GetSessionIdentifier(out IntPtr id);
    [PreserveSig] int GetSessionInstanceIdentifier(out IntPtr id);
    [PreserveSig] int GetProcessId(out uint pid);
}

public static class Sessions {
    /// State of the audio session the process has on the default render device:
    /// -1 none, 0 inactive, 1 active, 2 expired.
    public static int StateFor(uint pid) {
        var enumerator = (IMMDeviceEnumerator)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")));
        IMMDevice device;
        if (enumerator.GetDefaultAudioEndpoint(0, 1, out device) != 0 || device == null) return -1;
        Guid iid = typeof(IAudioSessionManager2).GUID;
        object instance;
        if (device.Activate(ref iid, 23, IntPtr.Zero, out instance) != 0) return -1;
        var manager = (IAudioSessionManager2)instance;
        IAudioSessionEnumerator sessions;
        if (manager.GetSessionEnumerator(out sessions) != 0) return -1;
        int count;
        sessions.GetCount(out count);
        int found = -1;
        for (int i = 0; i < count; i++) {
            IAudioSessionControl2 session;
            if (sessions.GetSession(i, out session) != 0 || session == null) continue;
            uint sessionPid;
            session.GetProcessId(out sessionPid);
            if (sessionPid != pid) continue;
            int state;
            session.GetState(out state);
            found = Math.Max(found, state);
        }
        return found;
    }
}
