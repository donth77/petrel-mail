@echo off
setlocal EnableExtensions
rem One command from source change to a relaunchable exe.
rem
rem There is no hot reload here: the vite dev server is not part of this
rem path, so the app must embed its assets. UI change -> run this -> quit
rem and reopen petrel-desktop.exe. Windows does not need an .app wrapper;
rem the exe is the app.
cd /d "%~dp0\.."

rem Point git at the tracked hooks. core.hooksPath is local config, so a fresh
rem clone has no hooks until something sets it; doing it here means the first
rem build arms the pre-commit fmt check. Idempotent, and quiet when already set.
set "HOOKS="
for /f "delims=" %%i in ('git config --get core.hooksPath 2^>nul') do set "HOOKS=%%i"
if not "%HOOKS%"==".githooks" git config core.hooksPath .githooks

rem Formatting is part of the build. This rewrites the tree rather than checking
rem it, which is convenient but was hiding unformatted commits: the reformat
rem landed in whatever commit came next, so anything committed before a rebuild
rem reached CI unformatted. .githooks/pre-commit is what actually catches that;
rem this line just means you rarely see it fire.
cargo fmt --all
if errorlevel 1 exit /b 1

pushd apps\desktop\ui
call pnpm run build
if errorlevel 1 (
  popd
  exit /b 1
)
popd

cargo build --release -p petrel-desktop --features custom-protocol
if errorlevel 1 exit /b 1

echo built: %CD%\target\release\petrel-desktop.exe
