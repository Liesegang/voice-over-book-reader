param([Parameter(Mandatory=$true)][string]$Bundle,[Parameter(Mandatory=$true)][string]$Work,[ValidateSet('general','images','fixed')][string]$Suite='general')
$ErrorActionPreference='Stop'
$testRoot=$PSScriptRoot
New-Item -ItemType Directory -Path $Work -Force | Out-Null
$fixture=Join-Path $Work 'source.xhtml'
$paragraph='<p>窓の外で、朝の風が<ruby>木立<rt>こだち</rt></ruby>を揺らしていた。「ここから先へ、歩いてみよう。」日本語の縦書きとルビ、閉じ括弧と句読点が自然に並ぶ。まだ誰も知らない道が続いている。</p>'
$xml='<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>Source regression</title><style>body{writing-mode:vertical-rl;font-family:"Yu Mincho",serif;font-size:24px;line-height:1.6}p{margin:0 0 0 1em;text-indent:1em}rt{font-size:.5em}</style></head><body><h1>原文のまま読む</h1><p>前<![CDATA[😀]]>後</p>'+($paragraph*180)+'</body></html>'
[IO.File]::WriteAllText($fixture,$xml,[Text.UTF8Encoding]::new($false))
function Format-FixtureScript([string]$Source) {
    # Our fixture sources have no multiline string indentation. Keep newlines
    # and inline comments (ASI), removing only indentation/comment-only lines
    # to fit Windows' per-environment-value limit. Production NAV stays exact.
    return (($Source -split "\r?\n" | Where-Object { $_ -notmatch '^\s*//' } | ForEach-Object { $_.TrimStart() }) -join "`n")
}
$navigatorPath=Join-Path $testRoot '../src/navigator.js'
$regressionPath=Join-Path $testRoot $(switch($Suite){'images'{'gaiji-regression.js'}'fixed'{'fixed-regression.js'}default{'chromium-regression.js'}})
$navigator=Format-FixtureScript ([IO.File]::ReadAllText($navigatorPath))
$regression=Format-FixtureScript ([IO.File]::ReadAllText($regressionPath))
$script="(()=>{$navigator`n$regression`nreturn 'started';})()"
if($script.Length -ge 32000){
    $minifier=$env:VOBR_ESBUILD
    if(!$minifier){$command=Get-Command esbuild -ErrorAction SilentlyContinue;if($command){$minifier=$command.Source}}
    if(!$minifier -or ![IO.File]::Exists($minifier)){throw 'Large fixture requires esbuild 0.25.12; set VOBR_ESBUILD to its executable. Production navigator is not minified.'}
    if((& $minifier --version) -ne '0.25.12'){throw 'Fixture minifier must be esbuild 0.25.12'}
    $navCompact=Join-Path $Work 'navigator.min.js';$testCompact=Join-Path $Work 'regression.min.js'
    & $minifier $navigatorPath --minify --charset=utf8 "--outfile=$navCompact"
    if($LASTEXITCODE -ne 0){throw 'Navigator test minification failed'}
    & $minifier $regressionPath --minify --charset=utf8 "--outfile=$testCompact"
    if($LASTEXITCODE -ne 0){throw 'Regression test minification failed'}
    $navigator=[IO.File]::ReadAllText($navCompact);$regression=[IO.File]::ReadAllText($testCompact)
    $script="(()=>{$navigator`n$regression`nreturn 'started';})()"
    if($script.Length -ge 32000){throw 'Minified fixture exceeds Windows environment value limit'}
}
$start=[Diagnostics.ProcessStartInfo]::new()
$start.FileName=Join-Path $Bundle 'demo-weld-win.exe'
$start.WorkingDirectory=$Bundle
$start.UseShellExecute=$false;$start.CreateNoWindow=$true
$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
$start.StandardOutputEncoding=[Text.Encoding]::UTF8;$start.StandardErrorEncoding=[Text.Encoding]::UTF8
$start.Environment['CEF_PATH']=$Bundle
$start.Environment['WELD_CACHE_ROOT']=Join-Path $Work 'cache'
$start.Environment['WELD_TIMEOUT_SECS']='8'
$start.Environment['WELD_URL']=[Uri]::new($fixture).AbsoluteUri
$start.Environment['WELD_SCRIPT']=$script
$start.Environment['RUST_LOG']='warn'
$start.Environment.Remove('WELD_BOOK_PROBE') | Out-Null
$start.Environment.Remove('WELD_PIXEL_FIXTURE') | Out-Null
$process=[Diagnostics.Process]::Start($start)
$stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
if(!$process.WaitForExit(20000)){$process.Kill($true);throw 'Chromium regression deadline exceeded'}
$output=$stdout.GetAwaiter().GetResult()+$stderr.GetAwaiter().GetResult()
[IO.File]::WriteAllText((Join-Path $Work 'chromium.log'),$output)
$output -split "`n" | Where-Object {$_ -match 'ConsoleMessage \{.*NAV_REGRESSION|panic|SCRIPT #.*=>'} | Select-Object -Last 8
$passed=$output -match 'ConsoleMessage \{[^\r\n]*message: "NAV_REGRESSION PASS'
$failed=$output -match 'ConsoleMessage \{[^\r\n]*message: "NAV_REGRESSION FAIL'
if($process.ExitCode -ne 0 -or !$passed -or $failed){throw 'Chromium source regression failed; see chromium.log'}
