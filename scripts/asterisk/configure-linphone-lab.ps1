param(
    [string]$Distro = "debian",
    [string]$AsteriskEnvFile = "/etc/asterisk/simadmin-lab.env",
    [string]$LinphoneConfig = (Join-Path $env:LOCALAPPDATA "Linphone\linphonerc"),
    [switch]$Restart
)

$ErrorActionPreference = "Stop"

function Read-LabEnvironment {
    param(
        [string]$WslDistro,
        [string]$Path
    )

    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = @(& wsl.exe -d $WslDistro -- cat $Path 2>$null)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    if ($exitCode -ne 0) {
        throw "Unable to read the Asterisk lab environment from WSL."
    }

    $values = @{}
    foreach ($line in $output) {
        if ($line -match '^([A-Z0-9_]+)=(.*)$') {
            $values[$Matches[1]] = $Matches[2].Trim()
        }
    }
    return $values
}

function Get-IniSections {
    param([string[]]$Lines)

    $sections = @()
    $current = $null
    for ($index = 0; $index -lt $Lines.Count; $index++) {
        if ($Lines[$index] -notmatch '^\[([^]]+)\]$') {
            continue
        }
        if ($null -ne $current) {
            $current.End = $index - 1
            $sections += $current
        }
        $current = [pscustomobject]@{
            Name = $Matches[1]
            Start = $index
            End = $Lines.Count - 1
        }
    }
    if ($null -ne $current) {
        $sections += $current
    }
    return $sections
}

function Get-IniValue {
    param(
        [string[]]$Lines,
        [object]$Section,
        [string]$Key
    )

    for ($index = $Section.Start + 1; $index -le $Section.End; $index++) {
        if ($Lines[$index] -match ('^' + [regex]::Escape($Key) + '=(.*)$')) {
            return $Matches[1]
        }
    }
    return $null
}

function Set-IniValue {
    param(
        [string[]]$Lines,
        [object]$Section,
        [string]$Key,
        [string]$Value
    )

    for ($index = $Section.Start + 1; $index -le $Section.End; $index++) {
        if ($Lines[$index] -match ('^' + [regex]::Escape($Key) + '=')) {
            $Lines[$index] = "$Key=$Value"
            return
        }
    }
    throw "The Linphone section '$($Section.Name)' is missing '$Key'."
}

$lab = Read-LabEnvironment -WslDistro $Distro -Path $AsteriskEnvFile
$required = @(
    "SIMADMIN_ASTERISK_TEST_PORT",
    "SIMADMIN_LINPHONE_USERNAME",
    "SIMADMIN_LINPHONE_SECRET"
)
foreach ($name in $required) {
    if (-not $lab.ContainsKey($name) -or [string]::IsNullOrWhiteSpace($lab[$name])) {
        throw "The Asterisk lab environment is missing '$name'."
    }
}

$username = $lab["SIMADMIN_LINPHONE_USERNAME"]
$secret = $lab["SIMADMIN_LINPHONE_SECRET"]
$port = $lab["SIMADMIN_ASTERISK_TEST_PORT"]
if ($username -notmatch '^[A-Za-z0-9._~-]+$' -or
    $secret -notmatch '^[A-Za-z0-9._~-]+$' -or
    $port -notmatch '^\d{1,5}$' -or
    [int]$port -lt 1 -or
    [int]$port -gt 65535) {
    throw "The Asterisk lab environment contains an invalid account or port."
}

$previousErrorAction = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $wslAddresses = ((& wsl.exe -d $Distro -- hostname -I 2>$null) -join " ").Trim()
    $exitCode = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousErrorAction
}
if ($exitCode -ne 0) {
    throw "Unable to resolve the current WSL address."
}
$hostAddress = @($wslAddresses -split '\s+' | Where-Object {
    $parsed = $null
    [System.Net.IPAddress]::TryParse($_, [ref]$parsed) -and
        $parsed.AddressFamily -eq [System.Net.Sockets.AddressFamily]::InterNetwork
}) | Select-Object -First 1
if (-not $hostAddress) {
    throw "WSL did not report an IPv4 address for the Asterisk lab."
}

if (-not (Test-Path -LiteralPath $LinphoneConfig -PathType Leaf)) {
    throw "Linphone configuration was not found."
}

$linphone = @(Get-Process -Name "linphone" -ErrorAction SilentlyContinue)
$linphoneExe = $linphone | ForEach-Object { $_.Path } | Where-Object { $_ } | Select-Object -First 1
if ($linphone.Count -gt 0 -and -not $Restart) {
    throw "Linphone is running. Re-run with -Restart so it cannot overwrite the updated account."
}
if ($Restart) {
    foreach ($process in $linphone) {
        Stop-Process -Id $process.Id
        $process.WaitForExit(10000) | Out-Null
    }
}

$lines = [System.IO.File]::ReadAllLines($LinphoneConfig)
$sections = Get-IniSections -Lines $lines
$proxySections = @($sections | Where-Object {
    $identity = Get-IniValue -Lines $lines -Section $_ -Key "reg_identity"
    $identity -match ('sips?:' + [regex]::Escape($username) + '@')
})
$authSections = @($sections | Where-Object {
    (Get-IniValue -Lines $lines -Section $_ -Key "username") -eq $username
})
if ($proxySections.Count -ne 1 -or $authSections.Count -ne 1) {
    throw "Expected exactly one matching Linphone proxy and authentication section."
}

$realm = "asterisk"
$ha1Material = [System.Text.Encoding]::UTF8.GetBytes("${username}:${realm}:${secret}")
$md5 = [System.Security.Cryptography.MD5]::Create()
try {
    $ha1 = -join ($md5.ComputeHash($ha1Material) | ForEach-Object { $_.ToString("x2") })
} finally {
    $md5.Dispose()
}

$proxy = $proxySections[0]
Set-IniValue -Lines $lines -Section $proxy -Key "reg_identity" -Value "sip:${username}@${hostAddress}"
Set-IniValue -Lines $lines -Section $proxy -Key "reg_proxy" -Value "sip:${hostAddress}:${port};transport=udp"
Set-IniValue -Lines $lines -Section $proxy -Key "reg_sendregister" -Value "1"

$auth = $authSections[0]
Set-IniValue -Lines $lines -Section $auth -Key "username" -Value $username
Set-IniValue -Lines $lines -Section $auth -Key "domain" -Value $hostAddress
Set-IniValue -Lines $lines -Section $auth -Key "realm" -Value $realm
Set-IniValue -Lines $lines -Section $auth -Key "algorithm" -Value "MD5"
Set-IniValue -Lines $lines -Section $auth -Key "ha1" -Value $ha1

$backup = "$LinphoneConfig.simadmin-backup"
if (-not (Test-Path -LiteralPath $backup)) {
    Copy-Item -LiteralPath $LinphoneConfig -Destination $backup
}
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($LinphoneConfig, (($lines -join "`n") + "`n"), $utf8NoBom)

if ($Restart) {
    if (-not $linphoneExe) {
        $candidate = "D:\Program\Chat\Linphone\bin\linphone.exe"
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $linphoneExe = $candidate
        }
    }
    if (-not $linphoneExe) {
        throw "Linphone was updated, but its executable could not be located for restart."
    }
    Start-Process -FilePath $linphoneExe
}

Write-Output "Linphone lab account updated for the current WSL Asterisk endpoint."
