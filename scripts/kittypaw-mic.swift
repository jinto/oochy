// KittyPaw Microphone Helper — Real-time speech-to-text
// Architecture inspired by Whispree (MIT License, github.com/Arsture/whispree)
//
// Usage: kittypaw-mic [--lang ko-KR] [--duration 10]
// Output: partial transcription results, one per line, flushed immediately
// Signal: SIGINT/SIGTERM to stop early

import Speech
import AVFoundation
import Foundation

// Parse arguments
var lang = "ko-KR"
var duration: Double = 10

let args = CommandLine.arguments
for i in 0..<args.count {
    if args[i] == "--lang" && i + 1 < args.count {
        lang = args[i + 1]
    }
    if args[i] == "--duration" && i + 1 < args.count {
        duration = Double(args[i + 1]) ?? 10
    }
}

// Setup audio engine (16kHz mono, inspired by Whispree AudioService)
let audioEngine = AVAudioEngine()
let recognizer = SFSpeechRecognizer(locale: Locale(identifier: lang))!
let request = SFSpeechAudioBufferRecognitionRequest()
request.shouldReportPartialResults = true

let node = audioEngine.inputNode
let recordingFormat = node.outputFormat(forBus: 0)

node.installTap(onBus: 0, bufferSize: 4096, format: recordingFormat) { buffer, _ in
    request.append(buffer)
}

// Graceful shutdown on SIGINT/SIGTERM
var isRunning = true
signal(SIGINT) { _ in isRunning = false }
signal(SIGTERM) { _ in isRunning = false }

// Start recording
audioEngine.prepare()
do {
    try audioEngine.start()
} catch {
    fputs("ERROR: Failed to start audio engine: \(error)\n", stderr)
    exit(1)
}

// Schedule auto-stop
DispatchQueue.main.asyncAfter(deadline: .now() + duration) {
    isRunning = false
}

// Start recognition
recognizer.recognitionTask(with: request) { result, error in
    if let result = result {
        let text = result.bestTranscription.formattedString
        // Print each partial result on its own line, flush immediately
        print(text)
        fflush(stdout)

        if result.isFinal {
            isRunning = false
        }
    }
    if error != nil && !isRunning {
        // Expected when we stop the engine
    } else if let error = error {
        fputs("ERROR: \(error.localizedDescription)\n", stderr)
        isRunning = false
    }
}

// Run loop until done
while isRunning {
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
}

// Cleanup
audioEngine.stop()
node.removeTap(onBus: 0)
request.endAudio()

// Brief delay to allow final result
RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.5))
exit(0)
