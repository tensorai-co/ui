//! Speech to text on macOS (`SFSpeechRecognizer` fed from the microphone),
//! because WKWebView doesn't implement the Web Speech API.
//!
//! Needs an app bundle whose Info.plist has `NSMicrophoneUsageDescription` and
//! `NSSpeechRecognitionUsageDescription` (`microphone_usage` / `speech_usage`
//! in `[package.metadata.ui]`), plus the `com.apple.security.device.audio-input`
//! entitlement when signed with the hardened runtime. Without the usage strings
//! macOS would kill the app, so [`listen`] returns an error instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend::macos_objc::{
    Id, NIL, class, msg, ns_string, rust_string, with_callback_block, with_repeat_block,
};
use crate::{Error, Result};

/// Recognized text so far; `is_final` when the recognizer is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub text: String,
    pub is_final: bool,
}

/// Speech-recognition permission (`SFSpeechRecognizerAuthorizationStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPermission {
    NotDetermined,
    Denied,
    Restricted,
    Authorized,
}

/// Whether the app may recognize speech, without asking.
pub fn permission() -> SpeechPermission {
    match msg!(isize; class("SFSpeechRecognizer"), "authorizationStatus") {
        1 => SpeechPermission::Denied,
        2 => SpeechPermission::Restricted,
        3 => SpeechPermission::Authorized,
        _ => SpeechPermission::NotDetermined,
    }
}

/// The usage strings macOS requires before touching the microphone or the
/// speech recognizer; `Err` names the missing ones.
pub fn check_usage_strings() -> Result<()> {
    let bundle = msg!(Id; class("NSBundle"), "mainBundle");
    let missing: Vec<&str> = [
        "NSMicrophoneUsageDescription",
        "NSSpeechRecognitionUsageDescription",
    ]
    .into_iter()
    .filter(|k| {
        bundle.is_null()
            || msg!(Id; bundle, "objectForInfoDictionaryKey:", ns_string(k) => Id).is_null()
    })
    .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidState(format!(
            "speech needs an app bundle whose Info.plist has {} (set microphone_usage / speech_usage in [package.metadata.ui])",
            missing.join(" and ")
        )))
    }
}

/// A running recognition; [`stop`](Self::stop) (or dropping it) ends it.
pub struct SpeechSession {
    engine: usize,
    request: usize,
    task: Arc<Mutex<usize>>,
    stopped: Arc<AtomicBool>,
}

impl SpeechSession {
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let (engine, request) = (self.engine as Id, self.request as Id);
        let input = msg!(Id; engine, "inputNode");
        msg!((); engine, "stop");
        if !input.is_null() {
            msg!((); input, "removeTapOnBus:", 0usize => usize);
        }
        msg!((); request, "endAudio");
        let task = *self.task.lock().unwrap_or_else(|e| e.into_inner()) as Id;
        if !task.is_null() {
            msg!((); task, "finish");
        }
    }
}

impl Drop for SpeechSession {
    fn drop(&mut self) {
        self.stop();
        msg!((); self.engine as Id, "release");
        msg!((); self.request as Id, "release");
    }
}

// SAFETY: the Objective-C objects are only messaged through thread-safe APIs
// (AVAudioEngine stop, request endAudio, task finish).
unsafe impl Send for SpeechSession {}

/// Asks for speech-recognition permission (the system prompt appears once).
pub fn request_permission(f: impl FnOnce(SpeechPermission) + Send + 'static) {
    if let Err(e) = check_usage_strings() {
        eprintln!("ui: {e}");
        f(SpeechPermission::Denied);
        return;
    }
    with_callback_block(
        move |status| {
            f(match status {
                1 => SpeechPermission::Denied,
                2 => SpeechPermission::Restricted,
                3 => SpeechPermission::Authorized,
                _ => SpeechPermission::NotDetermined,
            })
        },
        |block| msg!((); class("SFSpeechRecognizer"), "requestAuthorization:", block => Id),
    );
}

/// Starts listening on the default microphone. `on_text` gets partial and
/// final transcripts on a background thread; `locale` like `"en-US"`
/// (`None` uses the system language).
pub fn listen(
    locale: Option<&str>,
    on_text: impl FnMut(Result<Transcript>) + Send + 'static,
) -> Result<SpeechSession> {
    check_usage_strings()?;
    if permission() != SpeechPermission::Authorized {
        return Err(Error::InvalidState(
            "speech recognition isn't authorized; call ui::macos::speech::request_permission first"
                .into(),
        ));
    }
    let recognizer = match locale {
        Some(l) => {
            let loc =
                msg!(Id; class("NSLocale"), "localeWithLocaleIdentifier:", ns_string(l) => Id);
            msg!(Id; msg!(Id; class("SFSpeechRecognizer"), "alloc"), "initWithLocale:", loc => Id)
        }
        None => msg!(Id; msg!(Id; class("SFSpeechRecognizer"), "alloc"), "init"),
    };
    if recognizer.is_null() || !msg!(bool; recognizer, "isAvailable") {
        return Err(Error::Unsupported(
            "no speech recognizer is available for this language",
        ));
    }
    let request =
        msg!(Id; msg!(Id; class("SFSpeechAudioBufferRecognitionRequest"), "alloc"), "init");
    msg!((); request, "setShouldReportPartialResults:", true => bool);
    let engine = msg!(Id; msg!(Id; class("AVAudioEngine"), "alloc"), "init");
    let input = msg!(Id; engine, "inputNode");
    if input.is_null() {
        return Err(Error::Platform("no audio input device".into()));
    }
    let format = msg!(Id; input, "outputFormatForBus:", 0usize => usize);
    let req_addr = request as usize;
    with_repeat_block(
        move |buffer, _when| {
            if buffer != 0 {
                msg!((); req_addr as Id, "appendAudioPCMBuffer:", buffer as Id => Id);
            }
        },
        |block| {
            msg!((); input, "installTapOnBus:bufferSize:format:block:",
                0usize => usize, 1024u32 => u32, format => Id, block => Id);
        },
    );
    msg!((); engine, "prepare");
    let mut err: Id = NIL;
    if !msg!(bool; engine, "startAndReturnError:", &mut err as *mut Id => *mut Id) {
        msg!((); input, "removeTapOnBus:", 0usize => usize);
        return Err(Error::Platform("couldn't start the microphone".into()));
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let task = Arc::new(Mutex::new(0usize));
    let on_text = Arc::new(Mutex::new(on_text));
    let done = stopped.clone();
    with_repeat_block(
        move |result, error| {
            let mut cb = on_text.lock().unwrap_or_else(|e| e.into_inner());
            let result = result as Id;
            if !result.is_null() {
                let best = msg!(Id; result, "bestTranscription");
                let text = if best.is_null() {
                    String::new()
                } else {
                    rust_string(msg!(Id; best, "formattedString"))
                };
                let is_final = msg!(bool; result, "isFinal");
                cb(Ok(Transcript { text, is_final }));
            } else if error != 0 && !done.load(Ordering::Acquire) {
                let desc = rust_string(msg!(Id; error as Id, "localizedDescription"));
                cb(Err(Error::Platform(format!("speech recognition: {desc}"))));
            }
        },
        |block| {
            let t = msg!(Id; recognizer, "recognitionTaskWithRequest:resultHandler:", request => Id, block => Id);
            *task.lock().unwrap_or_else(|e| e.into_inner()) = msg!(Id; t, "retain") as usize;
        },
    );
    msg!((); recognizer, "release");
    Ok(SpeechSession {
        engine: engine as usize,
        request: request as usize,
        task,
        stopped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbundled_binaries_get_an_error_not_a_crash() {
        let err = check_usage_strings().unwrap_err().to_string();
        assert!(err.contains("NSMicrophoneUsageDescription"), "{err}");
        assert!(listen(None, |_| {}).is_err());
        // Reading the status never prompts.
        let _ = permission();
    }
}
