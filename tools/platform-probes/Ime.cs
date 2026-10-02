// Input-method helpers for the H3 probe (loaded with Add-Type by common.ps1 next to Probe.cs).
//
// - Tsf: switch the session's active keyboard input processor (what Win+Space does) and back.
//   The probe switches to Microsoft Pinyin for the run and always restores the previous one.
// - TargetIme: read and steer the IMM context of our own WinForms target (same process).
// - Screens: screenshots of a screen region, and the box of pixels that changed between two captures.
//   On Windows 11 the candidate window is drawn by TextInputHost: it is neither an enumerable
//   top-level window nor hit-testable through UI Automation, so the probe finds it by its pixels.

using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

[StructLayout(LayoutKind.Sequential)]
public struct TF_INPUTPROCESSORPROFILE {
    public uint dwProfileType;
    public ushort langid;
    public Guid clsid;
    public Guid guidProfile;
    public Guid catid;
    public IntPtr hklSubstitute;
    public uint dwCaps;
    public IntPtr hkl;
    public uint dwFlags;
}

[ComImport, Guid("71C6E74C-0F28-11D8-A82A-00065B84435C"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface ITfInputProcessorProfileMgr {
    [PreserveSig] int ActivateProfile(uint dwProfileType, ushort langid, ref Guid clsid, ref Guid guidProfile, IntPtr hkl, uint dwFlags);
    [PreserveSig] int DeactivateProfile(uint dwProfileType, ushort langid, ref Guid clsid, ref Guid guidProfile, IntPtr hkl, uint dwFlags);
    [PreserveSig] int GetProfile(uint dwProfileType, ushort langid, ref Guid clsid, ref Guid guidProfile, IntPtr hkl, out TF_INPUTPROCESSORPROFILE profile);
    [PreserveSig] int EnumProfiles(ushort langid, out IntPtr ppEnum);
    [PreserveSig] int ReleaseInputProcessor(ref Guid rclsid, uint dwFlags);
    [PreserveSig] int RegisterProfile(IntPtr a, IntPtr b, IntPtr c, IntPtr d, IntPtr e, IntPtr f, IntPtr g, IntPtr h, IntPtr i, IntPtr j, IntPtr k, IntPtr l);
    [PreserveSig] int UnregisterProfile(ref Guid rclsid, ushort langid, ref Guid guidProfile, uint dwFlags);
    [PreserveSig] int GetActiveProfile(ref Guid catid, out TF_INPUTPROCESSORPROFILE profile);
}

public static class Tsf {
    static readonly Guid CLSID_TF_InputProcessorProfiles = new Guid("33C53A50-F456-4884-B049-85FD643ECFED");
    static readonly Guid GUID_TFCAT_TIP_KEYBOARD = new Guid("34745C63-B2F0-4784-8B67-5E12C8701A31");
    public static readonly Guid MsPinyinClsid = new Guid("81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E");
    public static readonly Guid MsPinyinProfile = new Guid("FA550B04-5AD7-411F-A5AC-CA038EC515D7");

    static ITfInputProcessorProfileMgr Manager() {
        Type type = Type.GetTypeFromCLSID(CLSID_TF_InputProcessorProfiles);
        return (ITfInputProcessorProfileMgr)Activator.CreateInstance(type);
    }

    public static TF_INPUTPROCESSORPROFILE ActiveProfile() {
        Guid category = GUID_TFCAT_TIP_KEYBOARD;
        TF_INPUTPROCESSORPROFILE profile;
        Manager().GetActiveProfile(ref category, out profile);
        return profile;
    }

    public static bool IsMsPinyin(TF_INPUTPROCESSORPROFILE profile) { return profile.clsid == MsPinyinClsid; }

    /// Activates a keyboard TIP for the whole session. Returns the HRESULT.
    public static int Activate(Guid clsid, Guid profile, ushort langid) {
        return Manager().ActivateProfile(1, langid, ref clsid, ref profile, IntPtr.Zero, 0x4 | 0x20000000u);
    }

    public static string Describe(TF_INPUTPROCESSORPROFILE profile) {
        return String.Format("lang=0x{0:X} clsid={1} profile={2} name={3}", profile.langid, profile.clsid, profile.guidProfile,
            IsMsPinyin(profile) ? "Microsoft Pinyin" : "other");
    }
}

public static class TargetIme {
    [DllImport("imm32.dll")] static extern IntPtr ImmGetContext(IntPtr h);
    [DllImport("imm32.dll")] static extern bool ImmReleaseContext(IntPtr h, IntPtr himc);
    [DllImport("imm32.dll")] static extern bool ImmGetOpenStatus(IntPtr himc);
    [DllImport("imm32.dll")] static extern bool ImmSetOpenStatus(IntPtr himc, bool open);
    [DllImport("imm32.dll")] static extern bool ImmGetConversionStatus(IntPtr himc, out uint conversion, out uint sentence);
    [DllImport("imm32.dll")] static extern bool ImmSetConversionStatus(IntPtr himc, uint conversion, uint sentence);
    [DllImport("imm32.dll", CharSet = CharSet.Unicode)] static extern int ImmGetCompositionStringW(IntPtr himc, uint index, byte[] buffer, uint length);

    /// The composition string being typed in `window` (our own text box), or "" when none.
    public static string Composition(IntPtr window) {
        IntPtr context = ImmGetContext(window);
        if (context == IntPtr.Zero) return "";
        int bytes = ImmGetCompositionStringW(context, 8, null, 0);
        string text = "";
        if (bytes > 0) {
            var buffer = new byte[bytes];
            ImmGetCompositionStringW(context, 8, buffer, (uint)bytes);
            text = Encoding.Unicode.GetString(buffer);
        }
        ImmReleaseContext(window, context);
        return text;
    }

    /// Opens the input method in `window` and switches it to Chinese (native) mode.
    public static bool SetNative(IntPtr window) {
        IntPtr context = ImmGetContext(window);
        if (context == IntPtr.Zero) return false;
        uint conversion, sentence;
        ImmSetOpenStatus(context, true);
        ImmGetConversionStatus(context, out conversion, out sentence);
        bool ok = ImmSetConversionStatus(context, conversion | 1, sentence);
        ImmReleaseContext(window, context);
        return ok;
    }
}

public static class Screens {
    /// Captures a screen region (physical pixels).
    public static Bitmap Grab(int x, int y, int width, int height) {
        var bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        using (var graphics = Graphics.FromImage(bitmap)) {
            graphics.CopyFromScreen(x, y, 0, 0, new Size(width, height));
        }
        return bitmap;
    }

    /// Bounding box [left, top, right, bottom] (bitmap coordinates) of the pixels whose channels differ
    /// by more than `threshold` in total between the two captures, or null when none do.
    public static int[] ChangedBox(Bitmap before, Bitmap after, int threshold) {
        int width = Math.Min(before.Width, after.Width), height = Math.Min(before.Height, after.Height);
        var rect = new Rectangle(0, 0, width, height);
        BitmapData a = before.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        BitmapData b = after.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        try {
            var pa = new byte[a.Stride * height];
            var pb = new byte[b.Stride * height];
            Marshal.Copy(a.Scan0, pa, 0, pa.Length);
            Marshal.Copy(b.Scan0, pb, 0, pb.Length);
            int left = width, top = height, right = -1, bottom = -1;
            for (int y = 0; y < height; y++) {
                for (int x = 0; x < width; x++) {
                    int i = y * a.Stride + x * 4, j = y * b.Stride + x * 4;
                    int d = Math.Abs(pa[i] - pb[j]) + Math.Abs(pa[i + 1] - pb[j + 1]) + Math.Abs(pa[i + 2] - pb[j + 2]);
                    if (d <= threshold) continue;
                    left = Math.Min(left, x); right = Math.Max(right, x);
                    top = Math.Min(top, y); bottom = Math.Max(bottom, y);
                }
            }
            return right < 0 ? null : new int[] { left, top, right + 1, bottom + 1 };
        } finally {
            before.UnlockBits(a);
            after.UnlockBits(b);
        }
    }

    /// Saves a screenshot of a screen region (physical pixels) as PNG.
    public static bool Shot(string path, int x, int y, int width, int height) {
        try {
            using (var bitmap = Grab(x, y, width, height)) {
                bitmap.Save(path, ImageFormat.Png);
            }
            return true;
        } catch (Exception) {
            return false;
        }
    }
}
