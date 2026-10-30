param([Parameter(Mandatory=$true)][string]$RequestPath)
$ErrorActionPreference = 'Stop'
$utf8 = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $utf8
$request = Get-Content -LiteralPath $RequestPath -Raw -Encoding UTF8 | ConvertFrom-Json
try {
Add-Type -AssemblyName System.Speech
Add-Type -ReferencedAssemblies System.Speech -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Reflection;
using System.Speech.Synthesis;
using System.Speech.AudioFormat;
public sealed class ReaderSapiMark {
    public int position;
    public int length;
    public double seconds;
    public string text;
}
public static class ReaderSapi {
    // System.Speech AudioFileOut.Duration in Windows PowerShell's .NET
    // Framework divides converted output bytes by the engine INPUT byte rate.
    // Read the actual negotiated PCM formats and verify that exact condition;
    // do not estimate timing from text lengths or blindly stretch timestamps.
    // A future fixed runtime gets scale 1; unknown internals lose fine marks.
    private static double? AudioClockScale(SpeechSynthesizer synth) {
        try {
            BindingFlags flags = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic;
            object voice = typeof(SpeechSynthesizer).GetField("_voiceSynthesis", flags).GetValue(synth);
            object output = voice.GetType().GetField("_waveOut", flags).GetValue(voice);
            object inputFormat = output.GetType().GetField("_wfxIn", flags).GetValue(output);
            object outputFormat = output.GetType().GetField("_wfxOut", flags).GetValue(output);
            double inputRate = Convert.ToDouble(inputFormat.GetType().GetField("nAvgBytesPerSec", flags).GetValue(inputFormat));
            double outputRate = Convert.ToDouble(outputFormat.GetType().GetField("nAvgBytesPerSec", flags).GetValue(outputFormat));
            double bytes = Convert.ToDouble(output.GetType().GetField("_bytesWritten", flags).GetValue(output));
            double reported = ((TimeSpan)output.GetType().GetProperty("Duration", flags).GetValue(output, null)).TotalSeconds;
            if (inputRate <= 0 || outputRate <= 0 || bytes <= 0 || reported <= 0) return null;
            if (Math.Abs(reported - bytes / outputRate) < 0.001) return 1.0;
            if (Math.Abs(reported - bytes / inputRate) < 0.001) return inputRate / outputRate;
        } catch { }
        return null;
    }
    public static ReaderSapiMark[] Synthesize(string text, string voice, string path) {
        var marks = new List<ReaderSapiMark>();
        using (var synth = new SpeechSynthesizer()) {
            bool selected = false;
            if (!String.IsNullOrWhiteSpace(voice)) {
                foreach (var candidate in synth.GetInstalledVoices()) {
                    if (candidate.Enabled && String.Equals(candidate.VoiceInfo.Name, voice.Trim(), StringComparison.OrdinalIgnoreCase)) {
                        synth.SelectVoice(candidate.VoiceInfo.Name); selected = true; break;
                    }
                }
            }
            if (!selected) {
                foreach (var candidate in synth.GetInstalledVoices()) {
                    if (candidate.Enabled && candidate.VoiceInfo.Culture.TwoLetterISOLanguageName == "ja") {
                        synth.SelectVoice(candidate.VoiceInfo.Name); selected = true; break;
                    }
                }
                if (!selected) throw new InvalidOperationException("日本語のWindows音声がありません。音声を指定するかIrodoriを選択してください。");
            }
            synth.Rate = 0;
            synth.SetOutputToWaveFile(path);
            synth.SpeakProgress += (sender, e) => marks.Add(new ReaderSapiMark { position=e.CharacterPosition, length=e.CharacterCount, seconds=e.AudioPosition.TotalSeconds, text=e.Text });
            synth.Speak(text);
            double? clock = AudioClockScale(synth);
            if (clock.HasValue) {
                foreach (ReaderSapiMark mark in marks) mark.seconds *= clock.Value;
            } else {
                // Preserve playable PCM, but never advertise unverified word
                // times when the installed framework's clock is unfamiliar.
                marks.Clear();
            }
        }
        return marks.ToArray();
    }
}
'@
$marks = [ReaderSapi]::Synthesize([string]$request.text, [string]$request.voice, [string]$request.wav)
$json = if ($null -eq $marks -or @($marks).Count -eq 0) { '[]' } else { ConvertTo-Json -InputObject @($marks) -Compress }
[System.IO.File]::WriteAllText([string]$request.result, $json, (New-Object System.Text.UTF8Encoding($false)))
} catch {
    $detail = $_.Exception.ToString()
    if ($request.error) {
        [System.IO.File]::WriteAllText([string]$request.error, $detail, $utf8)
    }
    [Console]::Error.WriteLine($detail)
    exit 1
}
