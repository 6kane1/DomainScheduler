using System.Runtime.Versioning;
using System.Security.AccessControl;
using System.Security.Principal;

namespace DomainScheduler;

/// <summary>
/// The state folder (1.15.0 Test 1.3): %ProgramData%\Domain OS Toolbox. The program runs with administrator rights
/// and acts on what is kept here (backups it restores at start, the session it recovers, the boot guard, update
/// downloads), so only SYSTEM and the Administrators group may change anything in it. Users may read it (the log and
/// a downloaded playbook have to open in Explorer); they cannot create, change, rename or delete anything.
///
/// The folder sits directly under ProgramData, where a standard user may create folders but may not delete or rename
/// anyone else's. Before the first use in a run its owner and permissions are checked:
///  - missing: created with the protected permissions in the same call (no moment in which anybody else can open it);
///  - owned by SYSTEM / Administrators / TrustedInstaller and nobody else can write: used; extra read-only entries
///    (Explorer's "Continue" button adds one) are reset, the contents stay trusted;
///  - anything else (another owner, a write entry for anybody else, inherited permissions, a link or a file with
///    that name): somebody else could have put files there, so the whole thing is renamed to
///    "Domain OS Toolbox.untrusted-&lt;time&gt;", never read, and a fresh folder is created.
/// When none of that works the folder is not used at all: every load returns "nothing saved" and every save fails.
/// </summary>
public static class StateDir
{
    public const string FolderName = "Domain OS Toolbox";

    const string SystemSid = "S-1-5-18", AdminsSid = "S-1-5-32-544", UsersSid = "S-1-5-32-545",
                 TrustedInstallerSid = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

    /// <summary>Rights that change something: write/append data (= add file / add folder), extended attributes,
    /// delete a child, attributes, delete, permissions, owner, and the generic write/all bits (inherit-only entries).</summary>
    public const int WriteMask = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x10000 | 0x40000 | 0x80000 | 0x2000000 | 0x10000000 | 0x40000000;
    const int FullControl = 0x1F01FF, ReadAndExecute = 0x1200A9;
    const int InheritBoth = 0x1 | 0x2;     // OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE

    /// <summary>One entry of a folder's permission list (enough to judge it).</summary>
    public readonly record struct Ace(string Sid, int Mask, bool Allow, int Flags, bool Inherited);

    public enum Trust { Trusted, ResetPermissions, Untrusted }

    static readonly object _gate = new();
    static string _ready;
    static string _error;
    static readonly List<string> _notes = new();

    /// <summary>Where the folder is (nothing is checked).</summary>
    public static string PathOnly => Path.Combine(Environment.GetFolderPath(OperatingSystem.IsWindows()
        ? Environment.SpecialFolder.CommonApplicationData : Environment.SpecialFolder.LocalApplicationData), FolderName);   // not Windows: tests only

    /// <summary>Why the folder cannot be used in this run (null when it can or was not tried yet).</summary>
    public static string Error { get { lock (_gate) return _error; } }

    /// <summary>What the check did (moved an untrusted folder away, reset the permissions): read once for the log.</summary>
    public static List<string> TakeNotes()
    {
        lock (_gate)
        {
            var l = new List<string>(_notes);
            _notes.Clear();
            return l;
        }
    }

    /// <summary>
    /// The folder, checked and made safe (once per run). Throws when it cannot be made safe: then nothing in it is
    /// read or written in this run.
    /// </summary>
    public static string Ensure()
    {
        lock (_gate)
        {
            if (_ready != null) return _ready;
            if (_error != null) throw new UnauthorizedAccessException(_error);
            string p = PathOnly;
            try
            {
                if (OperatingSystem.IsWindows()) Secure(p);
                else Directory.CreateDirectory(p);
                _ready = p;
                return p;
            }
            catch (Exception e)
            {
                _error = L.F("The state folder {0} is not safe to use: {1}", p, e.Message);
                throw new UnauthorizedAccessException(_error, e);
            }
        }
    }

    /// <summary>
    /// The judgement of a folder's owner and permissions (pure). Untrusted: another owner, no permission list, or an
    /// allow entry that lets anybody but SYSTEM, Administrators or TrustedInstaller change something. ResetPermissions:
    /// safe, but not exactly ours (not protected from inheritance, extra or missing read entries).
    /// </summary>
    public static Trust Judge(string ownerSid, bool daclPresent, bool daclProtected, IReadOnlyList<Ace> aces)
    {
        if (!IsTrustedSid(ownerSid) || !daclPresent || aces == null) return Trust.Untrusted;
        foreach (var a in aces)
            if (a.Allow && !IsTrustedSid(a.Sid) && (a.Mask & WriteMask) != 0) return Trust.Untrusted;
        if (!daclProtected || aces.Any(a => a.Inherited)) return Trust.ResetPermissions;
        var want = Canonical().Select(a => (a.Sid, a.Mask, a.Allow, a.Flags)).OrderBy(x => x.Sid, StringComparer.Ordinal).ToList();
        var got = aces.Select(a => (a.Sid, a.Mask, a.Allow, a.Flags & InheritBoth)).OrderBy(x => x.Sid, StringComparer.Ordinal).ToList();
        return want.SequenceEqual(got) ? Trust.Trusted : Trust.ResetPermissions;
    }

    static bool IsTrustedSid(string sid) => sid is SystemSid or AdminsSid or TrustedInstallerSid;

    /// <summary>The permission list the folder gets: SYSTEM and Administrators full control, Users read; inherited by everything inside.</summary>
    public static IReadOnlyList<Ace> Canonical() => new[]
    {
        new Ace(SystemSid, FullControl, true, InheritBoth, false),
        new Ace(AdminsSid, FullControl, true, InheritBoth, false),
        new Ace(UsersSid, ReadAndExecute, true, InheritBoth, false),
    };

    // ---- Windows -------------------------------------------------------------------------------------------

    [SupportedOSPlatform("windows")]
    static DirectorySecurity Wanted(bool owner = true)
    {
        var s = new DirectorySecurity();
        if (owner) s.SetOwner(new SecurityIdentifier(AdminsSid));
        s.SetAccessRuleProtection(true, false);
        var inherit = InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit;
        s.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(SystemSid), FileSystemRights.FullControl, inherit, PropagationFlags.None, AccessControlType.Allow));
        s.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(AdminsSid), FileSystemRights.FullControl, inherit, PropagationFlags.None, AccessControlType.Allow));
        s.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(UsersSid), FileSystemRights.ReadAndExecute, inherit, PropagationFlags.None, AccessControlType.Allow));
        return s;
    }

    [SupportedOSPlatform("windows")]
    static Trust Read(string p)
    {
        var sd = new DirectoryInfo(p).GetAccessControl(AccessControlSections.Owner | AccessControlSections.Access);
        var raw = new RawSecurityDescriptor(sd.GetSecurityDescriptorBinaryForm(), 0);
        var aces = new List<Ace>();
        if (raw.DiscretionaryAcl != null)
            foreach (GenericAce g in raw.DiscretionaryAcl)
            {
                if (g is not KnownAce k) { aces.Add(new Ace("?", WriteMask, true, 0, false)); continue; }   // object/callback entries: not ours
                bool allow = g.AceType is AceType.AccessAllowed or AceType.AccessAllowedObject or AceType.AccessAllowedCallback or AceType.AccessAllowedCallbackObject;
                aces.Add(new Ace(k.SecurityIdentifier.Value, k.AccessMask, allow, (int)g.AceFlags, (g.AceFlags & AceFlags.Inherited) != 0));
            }
        bool prot = (raw.ControlFlags & ControlFlags.DiscretionaryAclProtected) != 0;
        return Judge(raw.Owner?.Value, raw.DiscretionaryAcl != null, prot, aces);
    }

    [SupportedOSPlatform("windows")]
    static void Secure(string p)
    {
        FileAttributes? attrs = null;
        try { if (File.Exists(p) || Directory.Exists(p)) attrs = File.GetAttributes(p); }
        catch (Exception) { attrs = FileAttributes.ReparsePoint; }      // unreadable: treated like a link
        if (attrs == null && IsDanglingLink(p)) attrs = FileAttributes.ReparsePoint;
        if (attrs != null)
        {
            var a = attrs.Value;
            bool link = (a & FileAttributes.ReparsePoint) != 0, dir = (a & FileAttributes.Directory) != 0;
            Trust t;
            try { t = link || !dir ? Trust.Untrusted : Read(p); }
            catch (Exception) { t = Trust.Untrusted; }      // its permissions cannot even be read: somebody shut administrators out
            if (t == Trust.ResetPermissions)
            {
                // The owner is already SYSTEM / Administrators / TrustedInstaller here: only the list is replaced.
                new DirectoryInfo(p).SetAccessControl(Wanted(owner: false));
                lock (_notes) _notes.Add(L.T("State folder: its permissions were reset (only administrators can change it)"));
            }
            else if (t == Trust.Untrusted) MoveAway(p, dir && !link);
        }
        if (!Directory.Exists(p))
        {
            // Created with its permissions in the same call. Owner: the Administrators group (an elevated token's
            // default anyway); if Windows refuses to set it, the token's default owner is checked below.
            try { new DirectoryInfo(p).Create(Wanted()); }
            catch (Exception) when (!Directory.Exists(p)) { new DirectoryInfo(p).Create(Wanted(owner: false)); }
        }
        // Whatever Windows made of the list: nobody but SYSTEM / Administrators / TrustedInstaller may change anything.
        if ((File.GetAttributes(p) & FileAttributes.ReparsePoint) != 0 || Read(p) == Trust.Untrusted)
            throw new InvalidOperationException(L.T("its owner or permissions are not as they must be"));
    }

    static bool IsDanglingLink(string p)
    {
        try { return new FileInfo(p).LinkTarget != null || new DirectoryInfo(p).LinkTarget != null; }
        catch (Exception) { return false; }
    }

    /// <summary>A folder somebody else could write to: renamed (never read), so a fresh one can be made.</summary>
    static void MoveAway(string p, bool dir)
    {
        string to = p + ".untrusted-" + DateTime.UtcNow.ToString("yyyyMMdd-HHmmss", System.Globalization.CultureInfo.InvariantCulture);
        if (dir) Directory.Move(p, to);       // a link is moved as the link itself, its target is not touched
        else File.Move(p, to);
        lock (_notes) _notes.Add(L.F("State folder: {0} could be changed by other accounts; it was renamed to {1} and is not used", p, to));
    }

    /// <summary>Tests: forget the check of this run.</summary>
    internal static void ResetForTests()
    {
        lock (_gate) { _ready = null; _error = null; _notes.Clear(); }
    }
}
