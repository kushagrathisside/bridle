# Bridle: take a picture of the top-level window of a program that Bridle started (ADR-0021).
#
# Run by Bridle with -EncodedCommand. It takes its settings from environment variables Bridle sets and nothing else:
#   BRIDLE_CAP_PID       the process Bridle started (its children are searched too)
#   BRIDLE_CAP_WAIT_MS   how long to wait for a window to appear
#   BRIDLE_CAP_TITLE     optional text the window title must contain
#   BRIDLE_CAP_OUT       the PNG file to write (a path Bridle chose)
#   BRIDLE_CAP_MAXPIX    the most pixels the picture may have
# It looks only at windows owned by that process tree, writes one PNG, and prints one line of JSON. It sends nothing anywhere, starts
# nothing, and reads no file. The mouse and the keyboard are not touched.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class BridleWin {
  public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
  [DllImport("user32.dll")] static extern int GetWindowTextLength(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int command);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();

  public class Found { public IntPtr Handle; public string Title; public int Width, Height; public uint Pid; }

  public static List<Found> Windows(HashSet<uint> pids) {
    var found = new List<Found>();
    EnumWindows((hwnd, l) => {
      uint pid; GetWindowThreadProcessId(hwnd, out pid);
      if (!pids.Contains(pid) || !IsWindowVisible(hwnd)) return true;
      RECT r; if (!GetWindowRect(hwnd, out r)) return true;
      int w = r.Right - r.Left, h = r.Bottom - r.Top;
      if (w < 80 || h < 60) return true;
      int n = GetWindowTextLength(hwnd);
      var sb = new StringBuilder(n + 1);
      GetWindowText(hwnd, sb, sb.Capacity);
      found.Add(new Found { Handle = hwnd, Title = sb.ToString(), Width = w, Height = h, Pid = pid });
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
'@

[void][BridleWin]::SetProcessDPIAware()

function Fail($message) {
  Write-Output (@{ ok = $false; error = $message } | ConvertTo-Json -Compress)
  exit 0
}

$rootPid = [uint32]$env:BRIDLE_CAP_PID
$waitMs = [int]$env:BRIDLE_CAP_WAIT_MS
$titleNeedle = $env:BRIDLE_CAP_TITLE
$out = $env:BRIDLE_CAP_OUT
$maxPixels = [int64]$env:BRIDLE_CAP_MAXPIX
if (-not $out -or -not $rootPid) { Fail 'missing settings' }

function ProcessTree([uint32]$root) {
  $set = New-Object 'System.Collections.Generic.HashSet[uint32]'
  [void]$set.Add($root)
  try {
    $all = Get-CimInstance -ClassName Win32_Process -Property ProcessId, ParentProcessId
    $grew = $true
    while ($grew) {
      $grew = $false
      foreach ($p in $all) {
        if ($set.Contains([uint32]$p.ParentProcessId) -and -not $set.Contains([uint32]$p.ProcessId)) {
          [void]$set.Add([uint32]$p.ProcessId); $grew = $true
        }
      }
    }
  } catch { }
  return $set
}

function Pick() {
  $pids = ProcessTree $rootPid
  $windows = [BridleWin]::Windows($pids)
  if ($titleNeedle) {
    $windows = $windows | Where-Object { $_.Title.IndexOf($titleNeedle, [StringComparison]::OrdinalIgnoreCase) -ge 0 }
  }
  return $windows | Sort-Object { $_.Width * $_.Height } -Descending | Select-Object -First 1
}

$deadline = [DateTime]::UtcNow.AddMilliseconds($waitMs)
$chosen = $null
while ([DateTime]::UtcNow -lt $deadline) {
  $chosen = Pick
  if ($chosen) { break }
  Start-Sleep -Milliseconds 150
}
if (-not $chosen) { Fail 'no window appeared before the wait ended' }
# Let it finish painting, and take the window as it is then.
Start-Sleep -Milliseconds 500
$again = Pick
if ($again) { $chosen = $again }

if ([BridleWin]::IsIconic($chosen.Handle)) { [void][BridleWin]::ShowWindow($chosen.Handle, 9); Start-Sleep -Milliseconds 300 }
$rect = New-Object BridleWin+RECT
[void][BridleWin]::GetWindowRect($chosen.Handle, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -lt 1 -or $h -lt 1) { Fail 'the window has no size' }
if ([int64]$w * [int64]$h -gt $maxPixels) { Fail "the window is $w x $h, over the limit of $maxPixels pixels" }

function Sample($bitmap) {
  # A 64 x 64 grid of the picture: the share of the most common colour, and how many distinct colours there are.
  $counts = @{}
  $stepX = [Math]::Max(1, [int]($bitmap.Width / 64)); $stepY = [Math]::Max(1, [int]($bitmap.Height / 64))
  $total = 0
  for ($y = 0; $y -lt $bitmap.Height; $y += $stepY) {
    for ($x = 0; $x -lt $bitmap.Width; $x += $stepX) {
      $c = $bitmap.GetPixel($x, $y)
      $key = ($c.R -shr 3) * 1024 + ($c.G -shr 3) * 32 + ($c.B -shr 3)
      if ($counts.ContainsKey($key)) { $counts[$key]++ } else { $counts[$key] = 1 }
      $total++
    }
  }
  $top = ($counts.Values | Measure-Object -Maximum).Maximum
  return @{ share = [double]$top / [double]$total; distinct = $counts.Count }
}

$bitmap = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$method = 'printwindow'
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
# 2 = PW_RENDERFULLCONTENT: also asks windows that draw with the GPU (WebView2, Electron, DirectX) for their content.
$printed = [BridleWin]::PrintWindow($chosen.Handle, $dc, 2)
$graphics.ReleaseHdc($dc)
$graphics.Dispose()
$stats = Sample $bitmap
if (-not $printed -or $stats.share -ge 0.999) {
  # Some windows cannot print themselves: copy that part of the screen instead (it must be on screen and not covered).
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $graphics.Dispose()
  $method = 'screen'
  $stats = Sample $bitmap
}
$bitmap.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output (@{
  ok = $true; title = $chosen.Title; width = $w; height = $h; method = $method; pid = [int]$chosen.Pid
  top_colour_share = [Math]::Round($stats.share, 4); distinct_colours = $stats.distinct
} | ConvertTo-Json -Compress)
