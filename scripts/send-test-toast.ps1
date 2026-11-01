<#
.SYNOPSIS
Sends a real Windows toast notification, for testing Tile's notification popups.

.DESCRIPTION
Posts the toast as an installed app (Windows Terminal by default) rather than as this script, so
the notification has a real app identity: Tile resolves the popup's icon from the app's
`shell:AppsFolder` entry, which only works for an app Windows actually knows about. Passing an
app with no icon, or one that isn't installed, shows the notification with no icon, the same as
a real app without one.

.PARAMETER Title
The notification's title.

.PARAMETER Message
The notification's body text.

.PARAMETER AppId
The AUMID to post as. Defaults to Windows Terminal's, since it is installed on most machines and
has a distinct icon. Run `Get-StartApps` to list AUMIDs of other installed apps to try.

.PARAMETER Progress
A 0-100 progress value to show a progress bar instead of plain body text.

.EXAMPLE
powershell -File .\send-test-toast.ps1 -Title "Build done" -Message "All tests passed"

.EXAMPLE
powershell -File .\send-test-toast.ps1 -Title "Deploying" -Progress 60

.NOTES
Run with Windows PowerShell (`powershell`), not PowerShell 7 (`pwsh`): pwsh cannot load the
`Windows.UI.Notifications` WinRT types this script needs.
#>
param(
    [Parameter(Mandatory)]
    [string]$Title,
    [string]$Message = "",
    [string]$AppId = "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App",
    [ValidateRange(0, 100)]
    [int]$Progress = -1
)

[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null

$xml = [Windows.Data.Xml.Dom.XmlDocument]::new()
if ($Progress -ge 0) {
    $xml.LoadXml(@"
<toast>
  <visual>
    <binding template="ToastGeneric">
      <text>$Title</text>
      <text>$Message</text>
      <progress value="$($Progress / 100)" title="" status="" />
    </binding>
  </visual>
</toast>
"@)
}
else {
    $xml.LoadXml(@"
<toast>
  <visual>
    <binding template="ToastGeneric">
      <text>$Title</text>
      <text>$Message</text>
    </binding>
  </visual>
</toast>
"@)
}

$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($AppId).Show($toast)
