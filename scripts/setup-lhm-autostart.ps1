#requires -RunAsAdministrator
# Registers a scheduled task that auto-starts LibreHardwareMonitor at every
# logon with highest privileges (needed for MSR / pawn.io driver access so
# CPU temps are readable). Also launches LHM once now.
$ErrorActionPreference = 'Stop'

$lhm = "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\LibreHardwareMonitor.LibreHardwareMonitor_Microsoft.Winget.Source_8wekyb3d8bbwe\LibreHardwareMonitor.exe"
if (-not (Test-Path $lhm)) { throw "LHM not found at $lhm" }
$user = "$env:USERDOMAIN\$env:USERNAME"

$act = New-ScheduledTaskAction -Execute $lhm
$trg = New-ScheduledTaskTrigger -AtLogOn -User $user
$set = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -StartWhenAvailable -MultipleInstances IgnoreNew -Hidden
$prc = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$task = New-ScheduledTask -Action $act -Trigger $trg -Settings $set -Principal $prc -Description 'Auto-start LibreHardwareMonitor for glassbar TEMP'
Register-ScheduledTask -TaskName 'LibreHardwareMonitor (glassbar)' -InputObject $task -Force | Out-Null
Write-Host "Registered scheduled task 'LibreHardwareMonitor (glassbar)' at logon, highest privileges."

if (-not (Get-Process LibreHardwareMonitor -ErrorAction SilentlyContinue)) {
  Start-Process -FilePath $lhm -WindowStyle Hidden
  Write-Host "Launched LibreHardwareMonitor."
} else {
  Write-Host "LHM already running."
}

Start-Sleep -Seconds 4
try {
  $code = (Invoke-WebRequest -UseBasicParsing 'http://localhost:8085/data.json' -TimeoutSec 5).StatusCode
  Write-Host "Web server reachable (HTTP $code) — glassbar TEMP should appear within 10 seconds."
} catch {
  Write-Host "Web server not yet reachable — give it ~10 seconds, then check glassbar TEMP."
}
Read-Host "Press Enter to close"
