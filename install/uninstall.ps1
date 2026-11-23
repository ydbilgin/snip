# Remove snip for the current user.
# Usage: pwsh -NoProfile -File uninstall.ps1 [-Purge]
#   Run it in an ELEVATED PowerShell when the task \Laureth\Snip exists (only an administrator can stop and delete it).
# What it does:
#   1. Stops the scheduled task \Laureth\Snip and removes it; stops a running snip by its process.
#      Win+Shift+S then goes back to Snipping Tool at once.
#   2. Removes the HKCU Run value "Laureth snip" if present (an older install, or the Settings switch).
#   3. Removes %LOCALAPPDATA%\Programs\snip and, when elevated, %ProgramFiles%\snip.
#   4. With -Purge: also removes %APPDATA%\Laureth\snip (config) and %LOCALAPPDATA%\Laureth\snip (log, state, history).
# Screenshots in Pictures\Screenshots are never touched.
param([switch]$Purge)
$ErrorActionPreference = 'Stop'
$taskPath = '\Laureth\'
$taskName = 'Snip'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$task = Get-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue
if ($task -and -not $admin) {
    throw 'The task \Laureth\Snip exists: run this in an elevated PowerShell.'
}
if ($task) {
    Stop-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskPath $taskPath -TaskName $taskName -Confirm:$false
    Write-Host "task removed: $taskPath$taskName"
}
$dirs = @((Join-Path $env:LOCALAPPDATA 'Programs\snip'))
if ($admin) { $dirs += (Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'snip') }
foreach ($dir in $dirs) {
    $exe = Join-Path $dir 'snip.exe'
    Get-Process -Name snip -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $exe) } |
        ForEach-Object {
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
            $null = $_.WaitForExit(5000)
            Write-Host "stopped snip (PID $($_.Id))"
        }
}
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Get-ItemProperty -Path $key -Name 'Laureth snip' -ErrorAction SilentlyContinue) {
    Remove-ItemProperty -Path $key -Name 'Laureth snip'
    Write-Host "Run entry removed"
}
foreach ($dir in $dirs) {
    if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force; Write-Host "removed $dir" }
}
if ($Purge) {
    foreach ($d in @((Join-Path $env:APPDATA 'Laureth\snip'), (Join-Path $env:LOCALAPPDATA 'Laureth\snip'))) {
        if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force; Write-Host "removed $d" }
    }
}
