param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference='Stop'
$taskVersionInfo=[System.Diagnostics.FileVersionInfo]::GetVersionInfo((Resolve-Path -LiteralPath $Executable).Path)
@{productVersion=$taskVersionInfo.ProductVersion;fileVersion=$taskVersionInfo.FileVersion} | ConvertTo-Json -Compress
