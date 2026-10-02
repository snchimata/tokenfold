# TaskStart Hook
# PowerShell template for Windows hook execution.

try {
    $rawInput = [Console]::In.ReadToEnd()
    if ($rawInput) {
        $null = $rawInput | ConvertFrom-Json
    }
} catch {
    Write-Error "[TaskStart] Invalid JSON input: $($_.Exception.Message)"
}

@{
    cancel = $false
    contextModification = ""
    errorMessage = ""
} | ConvertTo-Json -Compress
