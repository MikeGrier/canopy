# Copyright (c) Michael Grier.

param([string]$Name = 'canopy-wsl')

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$toolchain = Join-Path $root '.scratch/cargo-wsl'
$bin = Join-Path $toolchain 'bin'
[System.IO.Directory]::CreateDirectory($bin) | Out-Null
[System.IO.Directory]::CreateDirectory((Join-Path $toolchain 'lib/rustlib')) | Out-Null
$cargo = Join-Path $bin 'cargo.exe'
rustc --edition=2024 (Join-Path $PSScriptRoot 'cargo-wsl.rs') -o $cargo
if ($LASTEXITCODE -ne 0) { throw 'The WSL cargo bridge did not compile.' }
[System.IO.File]::Copy($cargo, (Join-Path $bin 'rustc.exe'), $true)
rustup toolchain link $Name $toolchain
if ($LASTEXITCODE -ne 0) { throw 'Rustup could not link the WSL cargo bridge.' }
$tests = Join-Path $root '.scratch/cargo-wsl-tests'
[System.IO.Directory]::CreateDirectory($tests) | Out-Null
$manifest = @(
	'[package]',
	'name = "cargo-wsl-tests"',
	'version = "0.0.0"',
	'edition = "2024"',
	'',
	'[workspace]',
	'',
	'[[bin]]',
	'name = "cargo-wsl"',
	'path = "../../tools/cargo-wsl.rs"'
) -join "`n"
[System.IO.File]::WriteAllBytes((Join-Path $tests 'Cargo.toml'), [System.Text.UTF8Encoding]::new($false).GetBytes($manifest + "`n"))
Write-Output "Cargo MCP calls can select toolchain '$Name' to run inside WSL."