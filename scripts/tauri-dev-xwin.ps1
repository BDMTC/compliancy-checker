# Compliancy Checker — Tauri on Windows without VS Build Tools (xwin + portable LLVM)
# Machine paths used by the home smoke box. Adjust if toolchains move.
$ErrorActionPreference = "Stop"
$llvmBin = "C:\Temp\toolchains\llvm\clang+llvm-23.1.5-x86_64-pc-windows-msvc\bin"
$xwin = "C:\Temp\toolchains\xwin"
if (-not (Test-Path "$llvmBin\clang-cl.exe")) { throw "Missing portable LLVM at $llvmBin" }
if (-not (Test-Path "$xwin\crt")) { throw "Missing xwin splat at $xwin (run xwin splat first)" }
if (-not (Test-Path "$llvmBin\rc.exe")) { Copy-Item "$llvmBin\llvm-rc.exe" "$llvmBin\rc.exe" -Force }

$env:Path = "C:\Users\MrMoo\.cargo\bin;$llvmBin;$env:Path"
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-msvc"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = "lld-link"
$env:INCLUDE = "$xwin\crt\include;$xwin\sdk\include\um;$xwin\sdk\include\shared;$xwin\sdk\include\ucrt;$xwin\sdk\include\winrt"
$env:LIB = "$xwin\crt\lib\x86_64;$xwin\sdk\lib\um\x86_64;$xwin\sdk\lib\ucrt\x86_64"
$env:LIBPATH = $env:LIB
$env:RUSTFLAGS = "-Lnative=$xwin\crt\lib\x86_64 -Lnative=$xwin\sdk\lib\um\x86_64 -Lnative=$xwin\sdk\lib\ucrt\x86_64"
$env:CC = "clang-cl"; $env:CXX = "clang-cl"; $env:AR = "llvm-lib"
$env:CC_x86_64_pc_windows_msvc = "clang-cl"
$env:CXX_x86_64_pc_windows_msvc = "clang-cl"
$env:AR_x86_64_pc_windows_msvc = "llvm-lib"
$env:CC_PREFER_CLANG_CL_OVER_MSVC = "1"
$env:RC = "llvm-rc"
$env:RC_x86_64_pc_windows_msvc = "llvm-rc"

Set-Location $PSScriptRoot\..
Write-Host "xwin/LLVM env ready — starting npm run tauri dev"
npm run tauri dev
