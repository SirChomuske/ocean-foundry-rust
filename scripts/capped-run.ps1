<#
Ocean Foundry - capped test runner.

Runs ONE program under a hard memory cap, watches it while it runs, and keeps
a log of every run and a crash report for every failure. Every test run of
OceanFoundry-cli.exe or OceanFoundry.exe goes through this script.

  pwsh -NoProfile -File scripts\capped-run.ps1 [-CapMB 2048] [-TimeoutS 300] [-Name label] -- <program> [args...]

How it protects the PC
  - The program is started suspended and put in a Windows Job Object before
    it runs a single instruction. The job refuses any allocation past -CapMB
    (default 2048 MB), so the program fails, not Windows.
  - Every -IntervalMs (250 ms) it records the program's memory, CPU and the
    PC's free RAM and free commit (RAM + page file). If the PC's free RAM
    falls below -FloorMB or free commit below -CommitFloorMB, the run is
    stopped at once, whatever the cap says.
  - It will not start when the cap does not fit in what the PC has free.
  - A cap above 8192 MB needs -UserApproved: runs that big need the user's
    go-ahead first.
  - Error pop-ups are suppressed, and nothing the program started outlives
    the run.

Logs (default test-logs\ in the project)
  runs.csv                   one line per run: when, name, outcome, seconds, peak MB
  runs\<stamp>-<name>\       stdout.txt, stderr.txt, samples.csv, run.txt
  crashes\<stamp>-<name>.txt for a failed run: why, the command, memory peaks,
                             the last samples and the ends of its output

Exit code: the program's own when it exited (0 = success, 101 = Rust panic),
otherwise 120 memory cap reached, 121 PC memory ran low, 122 crashed (a
Windows error code), 123 runner refused to start it, 124 timed out.
#>
# The runner's own options come first; everything from the program on (or
# after a bare --) is passed to the program untouched, dashes and all.
$CapMB = 2048; $TimeoutS = 300; $Name = ''; $LogDir = ''; $FloorMB = 3072; $CommitFloorMB = 6144; $IntervalMs = 250; $UserApproved = $false
$Command = @()
$argv = @($args)
for ($i = 0; $i -lt $argv.Count; $i++) {
    $a = [string]$argv[$i]
    $next = { if ($script:i + 1 -ge $script:argv.Count) { [Console]::Error.WriteLine("capped-run: $a needs a value"); exit 123 }; $script:i++; [string]$script:argv[$script:i] }
    switch -Regex ($a) {
        '^--$'              { if ($i + 1 -lt $argv.Count) { $Command = @($argv[($i + 1)..($argv.Count - 1)]) }; $i = $argv.Count; break }
        '^-CapMB$'          { $CapMB = [int](& $next); continue }
        '^-TimeoutS$'       { $TimeoutS = [int](& $next); continue }
        '^-Name$'           { $Name = & $next; continue }
        '^-LogDir$'         { $LogDir = & $next; continue }
        '^-FloorMB$'        { $FloorMB = [int](& $next); continue }
        '^-CommitFloorMB$'  { $CommitFloorMB = [int](& $next); continue }
        '^-IntervalMs$'     { $IntervalMs = [int](& $next); continue }
        '^-UserApproved$'   { $UserApproved = $true; continue }
        '^-'                { [Console]::Error.WriteLine("capped-run: unknown option $a (put -- before the program)"); exit 123 }
        default             { $Command = @($argv[$i..($argv.Count - 1)]); $i = $argv.Count; break }
    }
}
$ErrorActionPreference = 'Stop'

if (-not ('OfCapped.Runner' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace OfCapped {
public static class Native {
    [StructLayout(LayoutKind.Sequential)] public struct BASIC { public long PerProcessUserTimeLimit, PerJobUserTimeLimit; public uint LimitFlags; public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize; public uint ActiveProcessLimit; public UIntPtr Affinity; public uint PriorityClass, SchedulingClass; }
    [StructLayout(LayoutKind.Sequential)] public struct IOC { public ulong a, b, c, d, e, f; }
    [StructLayout(LayoutKind.Sequential)] public struct EXT { public BASIC Basic; public IOC Io; public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed; }
    [StructLayout(LayoutKind.Sequential)] public struct ACCT { public long TotalUserTime, TotalKernelTime, ThisPeriodTotalUserTime, ThisPeriodTotalKernelTime; public uint TotalPageFaultCount, TotalProcesses, ActiveProcesses, TotalTerminatedProcesses; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct STARTUPINFO { public int cb; public string lpReserved, lpDesktop, lpTitle; public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags; public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError; }
    [StructLayout(LayoutKind.Sequential)] public struct PI { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [StructLayout(LayoutKind.Sequential)] public struct SA { public int nLength; public IntPtr lpSecurityDescriptor; public int bInheritHandle; }
    [StructLayout(LayoutKind.Sequential)] public struct MEM { public uint dwLength, dwMemoryLoad; public ulong TotalPhys, AvailPhys, TotalPageFile, AvailPageFile, TotalVirtual, AvailVirtual, AvailExtendedVirtual; }
    [StructLayout(LayoutKind.Sequential)] public struct ACP { public IntPtr CompletionKey, CompletionPort; }
    [StructLayout(LayoutKind.Sequential)] public struct PMC { public uint cb, PageFaultCount; public UIntPtr PeakWorkingSetSize, WorkingSetSize, QuotaPeakPagedPoolUsage, QuotaPagedPoolUsage, QuotaPeakNonPagedPoolUsage, QuotaNonPagedPoolUsage, PagefileUsage, PeakPagefileUsage, PrivateUsage; }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern IntPtr CreateJobObjectW(IntPtr sa, string name);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetInformationJobObject(IntPtr job, int cls, ref EXT info, int len);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool SetInformationJobObject(IntPtr job, int cls, ref ACP info, int len);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr CreateIoCompletionPort(IntPtr file, IntPtr port, UIntPtr key, uint threads);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetQueuedCompletionStatus(IntPtr port, out uint msg, out UIntPtr key, out IntPtr overlapped, uint ms);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool QueryInformationJobObject(IntPtr job, int cls, out EXT info, int len, IntPtr ret);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool QueryInformationJobObject(IntPtr job, int cls, out ACCT info, int len, IntPtr ret);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool AssignProcessToJobObject(IntPtr job, IntPtr proc);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool TerminateProcess(IntPtr proc, uint code);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFO si, out PI pi);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll")] public static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll")] public static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern IntPtr CreateFileW(string name, uint access, uint share, ref SA sa, uint disp, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GlobalMemoryStatusEx(ref MEM m);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool K32GetProcessMemoryInfo(IntPtr proc, out PMC c, int cb);
}

public class Result {
    public uint ExitCode; public bool Exited; public string Stopped = ""; public int CapHits;
    public double Seconds, PeakPrivateMB, PeakWorkingMB, PeakJobMB, MinFreeRamMB, MinFreeCommitMB, StartFreeRamMB, StartFreeCommitMB;
}

public static class Runner {
    const double MB = 1024.0 * 1024.0;

    public static Native.MEM Memory() {
        var m = new Native.MEM(); m.dwLength = (uint)Marshal.SizeOf(typeof(Native.MEM));
        Native.GlobalMemoryStatusEx(ref m); return m;
    }

    // Quoting by the rules the C runtime uses to split a command line.
    public static string Quote(string a) {
        if (a.Length > 0 && a.IndexOfAny(new[] { ' ', '\t', '"' }) < 0) return a;
        var sb = new StringBuilder("\""); int slashes = 0;
        foreach (char c in a) {
            if (c == '\\') { slashes++; continue; }
            if (c == '"') { sb.Append('\\', slashes * 2 + 1).Append('"'); slashes = 0; continue; }
            sb.Append('\\', slashes).Append(c); slashes = 0;
        }
        return sb.Append('\\', slashes * 2).Append('"').ToString();
    }

    static IntPtr Open(string path, bool write) {
        var sa = new Native.SA { nLength = Marshal.SizeOf(typeof(Native.SA)), bInheritHandle = 1 };
        IntPtr h = write ? Native.CreateFileW(path, 0x40000000, 3, ref sa, 2, 0x80, IntPtr.Zero)
                         : Native.CreateFileW(path, 0x80000000, 3, ref sa, 3, 0, IntPtr.Zero);
        if (h == new IntPtr(-1)) throw new Exception("cannot open " + path + " (error " + Marshal.GetLastWin32Error() + ")");
        return h;
    }

    public static Result Run(string exe, string[] args, string cwd, ulong cap, int timeoutS, ulong floorRam, ulong floorCommit,
                             string outPath, string errPath, string samplesPath, int intervalMs) {
        var r = new Result();
        IntPtr job = Native.CreateJobObjectW(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Exception("cannot create a job object (error " + Marshal.GetLastWin32Error() + ")");
        var ext = new Native.EXT();
        // Process memory, job memory, die on an unhandled exception (no
        // error pop-up), kill everything when the job closes.
        ext.Basic.LimitFlags = 0x100 | 0x200 | 0x400 | 0x2000;
        ext.ProcessMemoryLimit = new UIntPtr(cap); ext.JobMemoryLimit = new UIntPtr(cap);
        if (!Native.SetInformationJobObject(job, 9, ref ext, Marshal.SizeOf(typeof(Native.EXT))))
            throw new Exception("cannot set the memory cap (error " + Marshal.GetLastWin32Error() + ")");
        // The job reports every allocation it refuses to this port.
        IntPtr port = Native.CreateIoCompletionPort(new IntPtr(-1), IntPtr.Zero, UIntPtr.Zero, 1);
        var acp = new Native.ACP { CompletionKey = IntPtr.Zero, CompletionPort = port };
        Native.SetInformationJobObject(job, 7, ref acp, Marshal.SizeOf(typeof(Native.ACP)));

        IntPtr hin = Open("NUL", false), hout = Open(outPath, true), herr = Open(errPath, true);
        var si = new Native.STARTUPINFO { cb = Marshal.SizeOf(typeof(Native.STARTUPINFO)), dwFlags = 0x100, hStdInput = hin, hStdOutput = hout, hStdError = herr };
        var cmd = new StringBuilder(Quote(exe));
        foreach (var a in args) cmd.Append(' ').Append(Quote(a));
        Native.PI pi;
        // Suspended, so it is in the job before it runs at all; no console window.
        if (!Native.CreateProcessW(null, cmd, IntPtr.Zero, IntPtr.Zero, true, 0x4 | 0x08000000, IntPtr.Zero, cwd, ref si, out pi)) {
            int e = Marshal.GetLastWin32Error(); Native.CloseHandle(hin); Native.CloseHandle(hout); Native.CloseHandle(herr);
            throw new Exception("cannot start " + exe + " (error " + e + ")");
        }
        Native.CloseHandle(hin); Native.CloseHandle(hout); Native.CloseHandle(herr);
        if (!Native.AssignProcessToJobObject(job, pi.hProcess)) {
            int e = Marshal.GetLastWin32Error(); Native.TerminateProcess(pi.hProcess, 1);
            throw new Exception("cannot put the program under the cap (error " + e + "); it was not run");
        }
        Native.ResumeThread(pi.hThread);

        var start = Memory();
        r.StartFreeRamMB = r.MinFreeRamMB = start.AvailPhys / MB;
        r.StartFreeCommitMB = r.MinFreeCommitMB = start.AvailPageFile / MB;
        var clock = Stopwatch.StartNew();
        int cores = Environment.ProcessorCount; long lastCpu = 0; double lastT = 0;
        using (var csv = new StreamWriter(samplesPath, false, new UTF8Encoding(false))) {
            csv.WriteLine("seconds,private_mb,working_set_mb,cpu_percent,processes,pc_free_ram_mb,pc_free_commit_mb");
            while (true) {
                uint wait = Native.WaitForSingleObject(pi.hProcess, (uint)intervalMs);
                double t = clock.Elapsed.TotalSeconds;
                Native.PMC pmc; Native.ACCT acct;
                Native.K32GetProcessMemoryInfo(pi.hProcess, out pmc, Marshal.SizeOf(typeof(Native.PMC)));
                Native.QueryInformationJobObject(job, 1, out acct, Marshal.SizeOf(typeof(Native.ACCT)), IntPtr.Zero);
                long cpu = acct.TotalUserTime + acct.TotalKernelTime;
                double cpuPct = t > lastT ? (cpu - lastCpu) / 1e7 / (t - lastT) / cores * 100.0 : 0.0;
                lastCpu = cpu; lastT = t;
                var m = Memory();
                double priv = (ulong)pmc.PrivateUsage / MB, ws = (ulong)pmc.WorkingSetSize / MB;
                double freeRam = m.AvailPhys / MB, freeCommit = m.AvailPageFile / MB;
                r.PeakPrivateMB = Math.Max(r.PeakPrivateMB, priv); r.PeakWorkingMB = Math.Max(r.PeakWorkingMB, ws);
                r.MinFreeRamMB = Math.Min(r.MinFreeRamMB, freeRam); r.MinFreeCommitMB = Math.Min(r.MinFreeCommitMB, freeCommit);
                csv.WriteLine(string.Format(CultureInfo.InvariantCulture, "{0:F2},{1:F0},{2:F0},{3:F0},{4},{5:F0},{6:F0}", t, priv, ws, cpuPct, acct.ActiveProcesses, freeRam, freeCommit));
                csv.Flush();
                uint msg; UIntPtr key; IntPtr ov;
                while (Native.GetQueuedCompletionStatus(port, out msg, out key, out ov, 0)) {
                    if (msg == 9 || msg == 10) r.CapHits++; // process or job memory limit
                }
                if (wait == 0) break;
                string stop = "";
                if (timeoutS > 0 && t > timeoutS) stop = "timed out after " + timeoutS + " s";
                else if (m.AvailPhys < floorRam) stop = string.Format("the PC's free RAM fell to {0:F0} MB, under the {1:F0} MB floor", freeRam, floorRam / MB);
                else if (m.AvailPageFile < floorCommit) stop = string.Format("the PC's free commit (RAM + page file) fell to {0:F0} MB, under the {1:F0} MB floor", freeCommit, floorCommit / MB);
                if (stop != "") {
                    r.Stopped = stop;
                    Native.TerminateJobObject(job, 1);
                    Native.WaitForSingleObject(pi.hProcess, 10000);
                    break;
                }
            }
        }
        r.Seconds = clock.Elapsed.TotalSeconds;
        uint code; Native.GetExitCodeProcess(pi.hProcess, out code);
        r.ExitCode = code; r.Exited = r.Stopped == "";
        Native.EXT peak;
        if (Native.QueryInformationJobObject(job, 9, out peak, Marshal.SizeOf(typeof(Native.EXT)), IntPtr.Zero))
            r.PeakJobMB = (ulong)peak.PeakJobMemoryUsed / MB;
        Native.CloseHandle(pi.hThread); Native.CloseHandle(pi.hProcess);
        uint m2; UIntPtr k2; IntPtr o2;
        while (Native.GetQueuedCompletionStatus(port, out m2, out k2, out o2, 0)) { if (m2 == 9 || m2 == 10) r.CapHits++; }
        Native.CloseHandle(job); // kills anything the program left running
        Native.CloseHandle(port);
        return r;
    }
}
}
'@
}

function Fail([string]$why) { [Console]::Error.WriteLine("capped-run: $why"); exit 123 }

if (-not $Command -or -not $Command[0]) { Fail 'no program given. Usage: capped-run.ps1 [-CapMB 2048] [-TimeoutS 300] -- <program> [args...]' }
$exe = $Command[0]
$rest = if ($Command.Length -gt 1) { [string[]]$Command[1..($Command.Length - 1)] } else { [string[]]@() }
$resolved = Get-Command $exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $resolved) { Fail "cannot find the program '$exe'" }
$exePath = $resolved.Source

if ($CapMB -lt 64) { Fail "-CapMB $CapMB is too small" }
if ($CapMB -gt 8192 -and -not $UserApproved) { Fail "-CapMB $CapMB is above 8192 MB. Runs that big need the user's go-ahead; add -UserApproved only once they have given it." }
$mem = [OfCapped.Runner]::Memory()
$freeMB = [math]::Floor($mem.AvailPhys / 1MB)
$freeCommitMB = [math]::Floor($mem.AvailPageFile / 1MB)
if ($CapMB + $FloorMB -gt $freeMB) { Fail "the cap of $CapMB MB does not fit: the PC has $freeMB MB of RAM free and keeps $FloorMB MB back. Close something or lower -CapMB." }
if ($CapMB + $CommitFloorMB -gt $freeCommitMB) { Fail "the cap of $CapMB MB does not fit in the PC's free commit ($freeCommitMB MB, $CommitFloorMB MB kept back)." }

$root = Split-Path -Parent $PSScriptRoot
if (-not $LogDir) { $LogDir = Join-Path $root 'test-logs' }
if (-not $Name) { $Name = [IO.Path]::GetFileNameWithoutExtension($exePath) + $(if ($rest.Length) { '-' + $rest[0] } else { '' }) }
$safe = ($Name -replace '[^\w.-]+', '_')
if ($safe.Length -gt 60) { $safe = $safe.Substring(0, 60) }
$stamp = (Get-Date).ToString('yyyyMMdd-HHmmss-fff')
$runDir = Join-Path $LogDir "runs\$stamp-$safe"
New-Item -ItemType Directory -Force $runDir | Out-Null
$out = Join-Path $runDir 'stdout.txt'; $err = Join-Path $runDir 'stderr.txt'; $samples = Join-Path $runDir 'samples.csv'

$env:RUST_BACKTRACE = '1'
$started = Get-Date
try {
    $r = [OfCapped.Runner]::Run($exePath, $rest, (Get-Location).Path, [uint64]$CapMB * 1MB, $TimeoutS,
        [uint64]$FloorMB * 1MB, [uint64]$CommitFloorMB * 1MB, $out, $err, $samples, $IntervalMs)
} catch {
    Fail $_.Exception.Message
}

$stdoutText = if (Test-Path $out) { [IO.File]::ReadAllText($out) } else { '' }
$stderrText = if (Test-Path $err) { [IO.File]::ReadAllText($err) } else { '' }
$code = [int64]$r.ExitCode
$known = @{ 0xC0000409L = 'aborted (Rust abort or fast fail)'; 0xC0000005L = 'access violation'; 0xC00000FDL = 'stack overflow';
            0xC0000017L = 'out of memory'; 0xC000012DL = 'commit limit reached'; 0xC0000135L = 'a DLL is missing'; 0xC000013AL = 'interrupted' }

# What happened, in one line, and the exit code to give back.
$capHit = $r.CapHits -gt 0 -or $stderrText -match 'memory allocation of \d+ bytes failed'
if ($r.Stopped) {
    $outcome = "stopped: $($r.Stopped)"; $exit = if ($r.Stopped -like 'timed out*') { 124 } else { 121 }
} elseif ($code -eq 0) {
    $outcome = 'ok'; $exit = 0
} elseif ($capHit) {
    $outcome = "memory cap of $CapMB MB reached (exit 0x{0:X8})" -f $code; $exit = 120
} elseif ($code -eq 101) {
    $outcome = 'panicked (exit 101)'; $exit = 101
} elseif ($code -ge 0xC0000000L) {
    $what = $known[$code]; if (-not $what) { $what = 'crashed' }
    $outcome = "{0} (exit 0x{1:X8})" -f $what, $code; $exit = 122
} else {
    $outcome = "failed (exit $code)"; $exit = [int]($code % 256); if ($exit -eq 0) { $exit = 1 }
}

$cmdLine = (@($exePath) + $rest | ForEach-Object { [OfCapped.Runner]::Quote($_) }) -join ' '
$summary = @(
    "name:      $Name"
    "command:   $cmdLine"
    "folder:    $((Get-Location).Path)"
    "started:   $($started.ToString('yyyy-MM-dd HH:mm:ss'))"
    "seconds:   {0:F1}" -f $r.Seconds
    "outcome:   $outcome"
    "refused:   $($r.CapHits) allocation(s) refused by the cap"
    "cap:       $CapMB MB per process and in total; timeout $TimeoutS s"
    "peak:      {0:F0} MB committed, {1:F0} MB in RAM" -f $r.PeakJobMB, $r.PeakWorkingMB
    "PC free:   RAM {0:F0} MB at start, lowest {1:F0} MB; commit {2:F0} MB at start, lowest {3:F0} MB" -f $r.StartFreeRamMB, $r.MinFreeRamMB, $r.StartFreeCommitMB, $r.MinFreeCommitMB
    "samples:   $samples"
)
Set-Content -Path (Join-Path $runDir 'run.txt') -Value $summary -Encoding utf8

$csv = Join-Path $LogDir 'runs.csv'
if (-not (Test-Path $csv)) { Set-Content -Path $csv -Value 'started,name,outcome,exit,seconds,peak_mb,cap_mb,run_folder' -Encoding utf8 }
$row = '{0},"{1}","{2}",{3},{4:F1},{5:F0},{6},"{7}"' -f $started.ToString('yyyy-MM-dd HH:mm:ss'), ($Name -replace '"', "'"), ($outcome -replace '"', "'"), $exit, $r.Seconds, $r.PeakJobMB, $CapMB, $runDir
Add-Content -Path $csv -Value $row -Encoding utf8

$crash = $null
if ($exit -ne 0) {
    New-Item -ItemType Directory -Force (Join-Path $LogDir 'crashes') | Out-Null
    $crash = Join-Path $LogDir "crashes\$stamp-$safe.txt"
    $tail = { param($text, $n) ($text -split "`r?`n" | Select-Object -Last $n) -join "`n" }
    $lastSamples = (Get-Content $samples | Select-Object -Last 20) -join "`n"
    $report = ($summary -join "`n") + "`n`n--- last samples (" + $IntervalMs + " ms apart)`n" + $lastSamples +
        "`n`n--- end of stderr`n" + (& $tail $stderrText 60) + "`n`n--- end of stdout`n" + (& $tail $stdoutText 30) + "`n"
    Set-Content -Path $crash -Value $report -Encoding utf8
}

# Pass the program's output through, then say how the run went.
if ($stdoutText) { [Console]::Out.Write($stdoutText) }
if ($stderrText) { [Console]::Error.Write($stderrText) }
$line = "[capped-run] $outcome; {0:F1} s; peak {1:F0} MB of {2} MB; log {3}" -f $r.Seconds, $r.PeakJobMB, $CapMB, $runDir
if ($crash) { $line += "; crash report $crash" }
[Console]::Out.WriteLine($line)
exit $exit
