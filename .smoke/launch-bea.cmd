@echo off
set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
start "" cmd /c "C:\laragon\www\Bea\src-tauri\target\debug\bea.exe > C:\laragon\www\Bea\.smoke\bea_out.log 2>&1"
