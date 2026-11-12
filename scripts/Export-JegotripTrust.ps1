#requires -Version 5.1
<#
.SYNOPSIS
快速只读提取本人 root 手机的无忧行 App UUID 与独立 SDK 身份材料。
.DESCRIPTION
不申请新授信、不登录、不改业务、不停止/恢复 App、不改 UUID；不是导出授信证书。
只在项目 .local/ 下创建当前 Windows 用户与 SYSTEM 可读的私有目录。
.EXAMPLE
.\scripts\Export-JegotripTrust.ps1
.EXAMPLE
.\scripts\Export-JegotripTrust.ps1 -Model 24122RKC7C
.EXAMPLE
.\scripts\Export-JegotripTrust.ps1 -OfflineBackup .\.local\jegotrip\device-readonly-20261112\phone-root-export.zip
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$Serial,
    [string]$Model,
    [string]$AdbPath,
    [string]$OfflineBackup,
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$project = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$private = Join-Path $project '.local'
$python = Get-Command python -CommandType Application -ErrorAction Stop | Select-Object -First 1
$helper = Join-Path $PSScriptRoot 'jegotrip_root_trust.py'
if (-not (Test-Path -LiteralPath $helper -PathType Leaf)) { throw '离线提取模块不存在。' }
if ($OfflineBackup -and ($Serial -or $Model -or $AdbPath)) { throw '离线备份模式不能同时选择 ADB 设备。' }
if ($Serial -and $Serial -notmatch '^[A-Za-z0-9._:-]{1,128}$') { throw '设备序列号格式不受支持。' }
if ($Model -and $Model -notmatch '^[A-Za-z0-9_-]{1,128}$') { throw '设备型号格式不受支持。' }
if (-not $OutputDirectory) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $OutputDirectory = Join-Path $private ('jegotrip\trusted-phone\' + $stamp + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
}
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$prefix = [System.IO.Path]::GetFullPath($private).TrimEnd('\') + '\'
if (-not $output.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) { throw '输出目录必须位于本项目 .local/ 下。' }
if (Test-Path -LiteralPath $output) { throw '输出目录已存在；不会覆盖以前的私有材料，请换一个新目录。' }
$ancestor = [System.IO.DirectoryInfo]::new([System.IO.Path]::GetDirectoryName($output))
while ($ancestor -and $ancestor.FullName.StartsWith($project, [System.StringComparison]::OrdinalIgnoreCase)) {
    if ($ancestor.Exists -and (($ancestor.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) { throw '输出路径包含重解析点；未访问手机或写入材料。' }
    $ancestor = $ancestor.Parent
}
if (-not $PSCmdlet.ShouldProcess($output, '只读读取本人无忧行原 UUID/SDK 标识，并创建私有导出；不改变授信或业务')) { return }
[void][System.IO.Directory]::CreateDirectory($output)
# 只保护新目录，文件继承它的 ACL；不递归清除文件 ACL，避免空 DACL。
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$system = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18')
$acl = [System.IO.DirectoryInfo]::new($output).GetAccessControl([System.Security.AccessControl.AccessControlSections]::Access)
$acl.SetAccessRuleProtection($true, $false)
foreach ($existing in @($acl.GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier]))) {
    [void]$acl.RemoveAccessRuleSpecific($existing)
}
$inherit = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
foreach ($identity in @($sid, $system)) {
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new($identity, [System.Security.AccessControl.FileSystemRights]::FullControl,
        $inherit, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
}
[System.IO.DirectoryInfo]::new($output).SetAccessControl($acl)
$check = Get-Acl -LiteralPath $output
$rules = @($check.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if (-not $check.AreAccessRulesProtected -or -not ($rules | Where-Object { $_.IdentityReference.Value -eq $sid.Value }) -or
    @($rules | Where-Object { $_.AccessControlType -eq 'Allow' -and $_.IdentityReference.Value -notin @($sid.Value, 'S-1-5-18') }).Count -gt 0) {
    throw '私有目录 ACL 核验失败，未访问手机。'
}
$arguments = @('-B', '-X', 'utf8', $helper, '--output', $output)
if ($OfflineBackup) { $arguments += @('--offline-backup', [System.IO.Path]::GetFullPath($OfflineBackup)) }
else {
    if ($AdbPath) { $arguments += @('--adb', [System.IO.Path]::GetFullPath($AdbPath)) }
    if ($Serial) { $arguments += @('--serial', $Serial) }
    if ($Model) { $arguments += @('--model', $Model) }
}
$ErrorActionPreference = 'Continue'
& $python.Source @arguments
$code = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($code -ne 0) {
    throw '材料读取未完成。请检查本人手机的 USB 调试授权/root 权限；多设备时指定 -Model 或 -Serial。不会尝试另一设备、安装工具或申请授信。'
}
Write-Host ('已保存本地 UUID-only 文件：' + (Join-Path $output 'identity.json'))
Write-Host '在无忧行登录表单选择该文件；读取材料不代表服务器当前认可，仍需同账号/号码与明确操作确认。'
