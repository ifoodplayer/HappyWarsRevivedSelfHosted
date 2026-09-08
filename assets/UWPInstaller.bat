@echo off
>nul 2>&1 "%SYSTEMROOT%\system32\cacls.exe" "%SYSTEMROOT%\system32\config\system"
if '%errorlevel%' NEQ '0' (
    echo Requesting administrator privileges...
    goto UACPrompt
) else ( goto gotAdmin )
:UACPrompt
    echo Set UAC = CreateObject^("Shell.Application"^) > "%temp%\getadmin.vbs"
    echo UAC.ShellExecute "%~s0", "", "", "runas", 1 >> "%temp%\getadmin.vbs"
    "%temp%\getadmin.vbs"
    del "%temp%\getadmin.vbs"
    exit /B
:gotAdmin
title UWP Installer

echo Removing old Happy Wars installation...
powershell -Command "Get-AppxPackage -AllUsers -Name '082B9D96.HappyWars' | Remove-AppxPackage -AllUsers" 2>nul
timeout /t 2 /nobreak >nul

echo Enabling developer mode...
reg add "HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock" /t REG_DWORD /f /v "AllowDevelopmentWithoutDevLicense" /d "1" >nul 2>&1

echo Disabling appx signature file...
set file_check="%~dp0AppxSignature.p7x"
set file_check_new="AppxSignature.tmp"
if exist %file_check% (
    ren %file_check% %file_check_new%
)

echo Installing Happy Wars UWP...
powershell Add-AppxPackage '%~dp0AppxManifest.xml' -Register >nul 2>&1

echo Enabling loopback exemption...
CheckNetIsolation LoopbackExempt -a -n="082B9D96.HappyWars_2hc244hj94j2p"

echo Creating desktop shortcut...
powershell -Command "$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut([Environment]::GetFolderPath('Desktop') + '\Happy Wars.lnk'); $s.TargetPath = 'shell:AppsFolder\082B9D96.HappyWars_2hc244hj94j2p!App'; $s.Save()"

echo Disabling developer mode...
reg add "HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock" /t REG_DWORD /f /v "AllowDevelopmentWithoutDevLicense" /d "0" >nul 2>&1
echo.
echo Done!

echo Launching Happy Wars UWP
start shell:AppsFolder\082B9D96.HappyWars_2hc244hj94j2p!App
pause