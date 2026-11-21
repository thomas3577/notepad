# Registers this notepad for .txt files for the current user (HKCU, no admin needed):
# "New > Text Document" in Explorer, "Open with" entry, text file icon.
# Usage:  .\register.ps1            (after `cargo install --path .`)
#         .\register.ps1 -Uninstall
param(
    [string]$Exe = "$env:USERPROFILE\.cargo\bin\notepad.exe",
    [switch]$Uninstall
)

$classes = 'HKCU:\Software\Classes'
$progId = 'RustNotepad.txt'

if ($Uninstall) {
    Remove-Item "$classes\$progId" -Recurse -ErrorAction SilentlyContinue
    Remove-Item "$classes\.txt\ShellNew" -Recurse -ErrorAction SilentlyContinue
    Remove-ItemProperty "$classes\.txt\OpenWithProgids" -Name $progId -ErrorAction SilentlyContinue
    if ((Get-ItemProperty "$classes\.txt" -ErrorAction SilentlyContinue).'(default)' -eq $progId) {
        Set-ItemProperty "$classes\.txt" -Name '(default)' -Value ''
    }
    return
}

if (-not (Test-Path $Exe)) { throw "Nicht gefunden: $Exe (zuerst 'cargo install --path .')" }

# ProgID: name shown in "New" menu, icon, open command
New-Item "$classes\$progId\shell\open\command" -Force | Out-Null
New-Item "$classes\$progId\DefaultIcon" -Force | Out-Null
Set-ItemProperty "$classes\$progId" -Name '(default)' -Value 'Textdokument'
Set-ItemProperty "$classes\$progId\DefaultIcon" -Name '(default)' -Value '%SystemRoot%\System32\imageres.dll,-102' -Type ExpandString
Set-ItemProperty "$classes\$progId\shell\open\command" -Name '(default)' -Value "`"$Exe`" `"%1`""

# .txt -> ProgID, plus "New > Textdokument"
New-Item "$classes\.txt\ShellNew" -Force | Out-Null
New-Item "$classes\.txt\OpenWithProgids" -Force | Out-Null
Set-ItemProperty "$classes\.txt" -Name '(default)' -Value $progId
Set-ItemProperty "$classes\.txt" -Name 'Content Type' -Value 'text/plain'
Set-ItemProperty "$classes\.txt" -Name 'PerceivedType' -Value 'text'
Set-ItemProperty "$classes\.txt\ShellNew" -Name 'NullFile' -Value ''
Set-ItemProperty "$classes\.txt\OpenWithProgids" -Name $progId -Value ''

Write-Host "Registriert: $Exe"
