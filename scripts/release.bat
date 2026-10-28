@echo off
setlocal EnableExtensions
rem One command from a clean tree to signed Windows installers and an
rem updater manifest. The checks and the build live in release.ps1.
rem
rem   scripts\release.bat 1.0.0
rem
rem Authenticode, the update-signing key, and the version gates are
rem described at the top of release.ps1.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0release.ps1" %*
exit /b %ERRORLEVEL%
