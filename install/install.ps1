# Install snip for the current user.
# Usage:
#   pwsh -NoProfile -File install.ps1 [-Source <snip.exe>] [-RunAtLogon]
#   -Source defaults to snip.exe next to this script (the release zip), else ..\target\release\snip.exe.
#   -RunAtLogon needs an ELEVATED PowerShell (it registers a task that runs at the highest level).
# What it does:
#   Without -RunAtLogon: copies the exe to %LOCALAPPDATA%\Programs\snip\snip.exe (the old one is kept
#   as snip.exe.previous). Start it from there; it runs at the normal level.
#   With -RunAtLogon: copies the exe to %ProgramFiles%\snip\snip.exe, a folder only administrators can
#   change, and registers the scheduled task \Laureth\Snip: at logon of this user, Interactive,
#   RunLevel Highest, no time limit, runs on battery, a second start is ignored (IgnoreNew).
#   An elevated task must never start an exe that a normal-level program could replace, so the
#   elevated copy does not live in the user's profile.
#   The HKCU Run value "Laureth snip" is removed, so snip does not start twice.
# Why elevated: snip's low-level keyboard hook gets no key events while an elevated window (an
# administrator terminal, Task Manager) has the focus, so Win+Shift+S and Win+Shift+T did nothing
# there. A hook at the same level as that window hears the keys. Everything snip opens from the
# tray (Explorer, folders) is started at the normal level on purpose.
# Undo: uninstall.ps1
param(
    [string]$Source = '',
    [switch]$RunAtLogon
)
$ErrorActionPreference = 'Stop'
if (-not $Source) {
    $Source = Join-Path $PSScriptRoot 'snip.exe'
    if (-not (Test-Path -LiteralPath $Source)) { $Source = Join-Path $PSScriptRoot '..\target\release\snip.exe' }
}
$taskPath = '\Laureth\'
$taskName = 'Snip'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$existing = Get-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue
# An existing task is registered again, so it never keeps pointing at an older, user-writable copy.
if ($existing) { $RunAtLogon = $true }
if (($RunAtLogon -or $existing) -and -not $admin) {
    throw 'Run this in an elevated PowerShell: the task \Laureth\Snip runs at the highest level and only an administrator can register or stop it.'
}
if (-not (Test-Path -LiteralPath $Source)) { throw "snip.exe not found: $Source (pass -Source)" }
$userDir = Join-Path $env:LOCALAPPDATA 'Programs\snip'
$adminDir = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'snip'
$dir = if ($RunAtLogon) { $adminDir } else { $userDir }
$dest = Join-Path $dir 'snip.exe'
if ($existing) {
    Stop-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue
}
# Ask a running snip to quit through the new exe (never the old one: it may have been replaced),
# then stop what is left by its process id and wait until it has exited.
& $Source quit | Out-Host
foreach ($d in @($userDir, $adminDir)) {
    $old = Join-Path $d 'snip.exe'
    Get-Process -Name snip -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $old) } |
        ForEach-Object {
            if (-not $_.WaitForExit(3000)) {
                Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
                $null = $_.WaitForExit(5000)
                Write-Host "stopped snip (PID $($_.Id))"
            }
        }
}
if (Test-Path -LiteralPath $dest) {
    Copy-Item -LiteralPath $dest -Destination "$dest.previous" -Force
}
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item -LiteralPath $Source -Destination $dest -Force
Write-Host "copied $Source -> $dest"
Write-Host ("sha256 " + (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash)
if ($RunAtLogon) {
    $user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    $action = New-ScheduledTaskAction -Execute $dest
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskPath $taskPath -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
    Write-Host "task registered: $taskPath$taskName (at logon of $user, Interactive, Highest)"
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    if (Get-ItemProperty -Path $key -Name 'Laureth snip' -ErrorAction SilentlyContinue) {
        Remove-ItemProperty -Path $key -Name 'Laureth snip'
        Write-Host "old Run value 'Laureth snip' removed (it started snip at the normal level)"
    }
    Start-ScheduledTask -TaskPath $taskPath -TaskName $taskName
    Write-Host "started through the task"
    $oldUserExe = Join-Path $userDir 'snip.exe'
    if (Test-Path -LiteralPath $oldUserExe) {
        Remove-Item -LiteralPath $userDir -Recurse -Force
        Write-Host "removed the old user-level copy $userDir"
    }
} else {
    Write-Host "not registered for logon. Start it with: `"$dest`" (normal level: Win+Shift+S then fails above an elevated window). Use -RunAtLogon for the elevated task."
}
