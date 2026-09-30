//! Host-side music bypass for Geometry Dash 2.11.
//!
//! GD's music (menu loop and level tracks) goes through FMOD Ex's streaming
//! path: the game loads a whole MP3 into a guest buffer, hands it to FMOD and
//! frees it, expecting FMOD to keep decoding from memory on a worker thread
//! fed through Mach semaphores. Inside the emulator that pipeline livelocks
//! (the FMOD worker threads spin in a semaphore/usleep handshake and never
//! advance their ring buffer), so the music channel contributes silence while
//! one-shot sound effects — decoded before the buffer is freed — play fine.
//!
//! Rather than teach the emulator FMOD's whole threading contract, this
//! module watches guest `fopen()`s for MP3 tracks and plays them directly
//! with Symphonia-decoded PCM on a dedicated host thread with its own OpenAL
//! device. The guest's FMOD path keeps running (it still paces the game
//! logic); the bypass just fills the silence with the same audio the game
//! asked to play.



use crate::audio::AudioFile;
use crate::environment::Environment;
use crate::fs::GuestPath;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use touchHLE_openal_soft_wrapper::{al_defines as aldef, al_types as altypes};
use touchHLE_openal_soft_wrapper as al;

const AL_LOOPING: altypes::ALenum = 0x1007;
const AL_GAIN: altypes::ALenum = 0x100A;
const AL_SOURCE_RELATIVE: altypes::ALenum = 0x0202;
const AL_TRUE: altypes::ALint = 1;

/// Track extensions the bypass should intercept. GD streams its music as
/// `<bundle>/*.mp3`; one-shot SFX (decoded before their buffers are freed)
/// already work through the guest's own audio stack.
const BYPASS_SUFFIXES: [&str; 1] = [".mp3"];

enum PlayerCommand {
    Play {
        pcm: std::sync::Arc<Vec<u8>>,
        sample_rate: u32,
        channels: u32,
        name: String,
    },
    /// FMOD rewound a tracked music stream back to offset 0 (GD does this
    /// when restarting a level after death): restart the track from the top.
    Restart { name: String },
    /// FMOD read more data from a tracked music stream. Sustained reads mean
    /// the guest is consuming the track; a long read drought means the stream
    /// is paused (GD pause menu) and playback should pause too.
    Activity { name: String },
    /// FMOD closed a tracked music stream's `FILE*` (`fclose`): GD stopped
    /// the track (leaving a level, entering a level after the level-select
    /// preview, death + respawn re-opens the file). Stop immediately so the
    /// next `Play` for the same track starts from the top and stays in sync
    /// with the game, instead of layering over the old playback.
    Stop { name: String },
}

static PLAYER_SENDER: Mutex<Option<Sender<PlayerCommand>>> = Mutex::new(None);

/// Guest `FILE*` addresses of music streams currently opened by FMOD, mapped
/// to the track name they belong to. Used by the stdio hooks (`fread`/
/// `fseek`) to attribute stream activity to the bypass track.
static TRACKED_FILES: std::sync::OnceLock<Mutex<HashMap<u32, String>>> =
    std::sync::OnceLock::new();

fn tracked_files() -> &'static Mutex<HashMap<u32, String>> {
    TRACKED_FILES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember the guest `FILE*` behind a bypassed track so stdio hooks can
/// report activity for it.
pub fn register_music_file(file_ptr: u32, name: String) {
    tracked_files().lock().unwrap().insert(file_ptr, name);
}

pub fn unregister_music_file(file_ptr: u32) {
    let mut tracked = tracked_files().lock().unwrap();
    let removed = tracked.remove(&file_ptr);
    // GD frequently opens a fresh stream for a track before closing the
    // stale one; closing the duplicate must not silence the fresh copy
    // (this caused overlapping-then-dead menu music on scene changes).
    let still_open = removed
        .as_ref()
        .is_some_and(|name| tracked.values().any(|other| other == name));
    drop(tracked);
    if let Some(name) = removed {
        if !still_open {
            let sender = PLAYER_SENDER.lock().unwrap().clone();
            if let Some(sender) = sender {
                let _ = sender.send(PlayerCommand::Stop { name });
            }
        }
    }
}

fn tracked_name(file_ptr: u32) -> Option<String> {
    tracked_files().lock().unwrap().get(&file_ptr).cloned()
}

/// Called from `fread()` for every guest read; only tracked music streams
/// produce a command.
pub fn note_stdio_activity(file_ptr: u32) {
    if let Some(name) = tracked_name(file_ptr) {
        let sender = PLAYER_SENDER.lock().unwrap().clone();
        if let Some(sender) = sender {
            let _ = sender.send(PlayerCommand::Activity { name });
        }
    }
}

/// Called from `fseeko()` when a tracked stream is rewound to offset 0.
pub fn note_stdio_rewind(file_ptr: u32) {
    if let Some(name) = tracked_name(file_ptr) {
        let sender = PLAYER_SENDER.lock().unwrap().clone();
        if let Some(sender) = sender {
            let _ = sender.send(PlayerCommand::Restart { name });
        }
    }
}

/// Last decoded track. GD re-opens the same MP3 (especially the menu loop)
/// many times per session; re-decoding megabytes of PCM on the guest main
/// thread each time would stall emulation, so the most recent track is kept
/// alive here and re-shared through an `Arc`.
static PCM_CACHE: Mutex<Option<(String, std::sync::Arc<Vec<u8>>, u32, u32)>> = Mutex::new(None);

/// Called from `fopen()` for every guest-opened audio file while running GD.
/// Returns the bypassed track's name when the file was taken over, so the
/// caller can register the resulting `FILE*` with `register_music_file`.
pub fn on_music_file_open(env: &mut Environment, filename: &GuestPath) -> Option<String> {
    // Cheap suffix check first, so unrelated callers only pay a string
    // compare.
    let path_str = filename.as_str().to_ascii_lowercase();
    if !BYPASS_SUFFIXES
        .iter()
        .any(|suffix| path_str.ends_with(suffix))
    {
        return None;
    }

    let name = filename
        .as_str()
        .rsplit('/')
        .next()
        .unwrap_or(filename.as_str())
        .to_string();

    let cached = {
        let guard = PCM_CACHE.lock().unwrap();
        guard
            .as_ref()
            .filter(|(cached_name, _, _, _)| cached_name == &name)
            .map(|(_, pcm, rate, channels)| {
                (std::sync::Arc::clone(pcm), *rate, *channels)
            })
    };

    let (pcm, sample_rate, channels) = match cached {
        Some(data) => data,
        None => {
            let Some((pcm, sample_rate, channels)) = decode_track(env, filename, &name) else {
                return None;
            };
            let pcm = std::sync::Arc::new(pcm);
            *PCM_CACHE.lock().unwrap() =
                Some((name.clone(), std::sync::Arc::clone(&pcm), sample_rate, channels));
            (pcm, sample_rate, channels)
        }
    };

    let sender = ensure_player_thread();
    if let Err(err) = sender.send(PlayerCommand::Play {
        pcm,
        sample_rate,
        channels,
        name: name.clone(),
    }) {
        log!("music bypass: player thread died: {:?}", err);
    }
    Some(name)
}

fn decode_track(
    env: &mut Environment,
    filename: &GuestPath,
    name: &str,
) -> Option<(Vec<u8>, u32, u32)> {
    let audio_file = match AudioFile::open_for_reading(filename, &env.fs) {
        Ok(file) => file,
        Err(err) => {
            log!("music bypass: could not decode {}: {:?}", name, err);
            return None;
        }
    };
    match audio_file.into_decoded_pcm() {
        Some((pcm, sample_rate, channels)) => Some((pcm, sample_rate, channels)),
        None => {
            log!("music bypass: unsupported container for {}", name);
            None
        }
    }
}

fn ensure_player_thread() -> Sender<PlayerCommand> {
    let mut guard = PLAYER_SENDER.lock().unwrap();
    if let Some(sender) = guard.as_ref() {
        return sender.clone();
    }
    // Claim the sender slot BEFORE spawning: two FMOD threads can open the
    // same MP3 concurrently (loading screen + main menu both trigger
    // menuLoop.mp3), and if both see an empty slot they each spawn a player
    // thread, producing two overlapping copies of the track.
    let (sender, receiver) = channel::<PlayerCommand>();
    *guard = Some(sender.clone());
    std::thread::Builder::new()
        .name("GD music bypass".to_string())
        .spawn(move || {
            player_thread(receiver);
        })
        .expect("music bypass: failed to spawn player thread");
    sender
}

fn player_thread(receiver: Receiver<PlayerCommand>) {
    const AL_BUFFER: altypes::ALenum = 0x1009;
    const AL_STOPPED: altypes::ALint = 0x1014;
    const AL_BUFFERS_QUEUED: altypes::ALenum = 0x1015;
    const AL_BUFFERS_PROCESSED: altypes::ALenum = 0x1016;
    const CHUNK_BYTES: usize = 1024 * 1024;

    unsafe {
        let device = al::alcOpenDevice(std::ptr::null());
        if device.is_null() {
            log!("music bypass: alcOpenDevice failed");
            return;
        }
        let context = al::alcCreateContext(device, std::ptr::null());
        if context.is_null() {
            log!("music bypass: alcCreateContext failed");
            al::alcCloseDevice(device);
            return;
        }
        if al::alcMakeContextCurrent(context) == 0 {
            log!("music bypass: alcMakeContextCurrent failed");
            return;
        }

        const AL_PLAYING: altypes::ALint = 0x1012;
        // If the guest stops reading from the track's file for this long, FMOD
        // has paused/stopped the stream (GD pause menu, death screen): silence
        // the bypass too, and resume as soon as reads pick back up.
        const STREAM_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
        const PAUSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

        let mut source: altypes::ALuint = 0;
        let mut buffers: Vec<altypes::ALuint> = Vec::new();
        let mut current_name = String::new();
        let mut paused = false;
        let mut stopped = false;
        let mut streaming = false;
        let mut play_started = std::time::Instant::now();
        let mut last_activity = std::time::Instant::now();

        loop {
            match receiver.recv_timeout(std::time::Duration::from_millis(25)) {
                Ok(PlayerCommand::Play { pcm, sample_rate, channels, name }) => {
                    if name == current_name && source != 0 {
                        let mut state = 0;
                        al::alGetSourcei(source, aldef::AL_SOURCE_STATE, &mut state);
                        if state != AL_STOPPED {
                            // GD re-opens the same track on scene changes while
                            // it should keep playing; treat that as activity.
                            play_started = std::time::Instant::now();
                            continue;
                        }
                        stopped = false;
                    }
                    if source != 0 {
                        al::alSourceStop(source);
                        // Unqueue everything still attached before deleting:
                        // OpenAL refuses to delete buffers that are still
                        // queued on a source (AL_INVALID_VALUE).
                        let mut detach = 0;
                        al::alGetSourcei(source, AL_BUFFERS_QUEUED, &mut detach);
                        while detach > 0 {
                            let mut done = detach;
                            if done > 64 {
                                done = 64;
                            }
                            let mut scratch = vec![0u32; done as usize];
                            al::alSourceUnqueueBuffers(source, done, scratch.as_mut_ptr());
                            if al::alGetError() != aldef::AL_NO_ERROR {
                                break;
                            }
                            detach -= done;
                        }
                        al::alSourcei(source, AL_BUFFER, 0);
                        al::alDeleteSources(1, &source);
                        if !buffers.is_empty() {
                            al::alDeleteBuffers(buffers.len() as altypes::ALsizei, buffers.as_ptr());
                            buffers.clear();
                        }
                    }
                    // Flush any sticky error left over from earlier calls so
                    // the per-chunk checks below only report fresh failures.
                    al::alGetError();
                    al::alGenSources(1, &mut source);
                    al::alSourcei(source, AL_LOOPING, 0);
                    al::alSourcef(source, AL_GAIN, 1.0);
                    al::alSourcei(source, AL_SOURCE_RELATIVE, AL_TRUE);

                    let format = match channels {
                        1 => aldef::AL_FORMAT_MONO16,
                        2 => aldef::AL_FORMAT_STEREO16,
                        other => {
                            log!("music bypass: {} has {} channels, unsupported", name, other);
                            continue;
                        }
                    };
                    let frame_bytes = channels as usize * 2;
                    let chunk_bytes = (CHUNK_BYTES / frame_bytes) * frame_bytes;
                    let mut offset = 0usize;
                    while offset < pcm.len() {
                        let end = (offset + chunk_bytes).min(pcm.len());
                        let mut buffer = 0;
                        al::alGenBuffers(1, &mut buffer);
                        al::alBufferData(
                            buffer,
                            format,
                            pcm[offset..end].as_ptr().cast(),
                            (end - offset) as altypes::ALsizei,
                            sample_rate as altypes::ALsizei,
                        );
                        let error = al::alGetError();
                        if error != aldef::AL_NO_ERROR {
                            log!(
                                "music bypass: alBufferData {} chunk {} error {:#x}",
                                name,
                                buffers.len(),
                                error
                            );
                            al::alDeleteBuffers(1, &buffer);
                            break;
                        }
                        al::alSourceQueueBuffers(source, 1, &buffer);
                        buffers.push(buffer);
                        offset = end;
                    }
                    if buffers.is_empty() {
                        continue;
                    }
                    al::alSourcePlay(source);
                    current_name = name.clone();
                    paused = false;
                    stopped = false;
                    // A track is only "streaming" (pausable on guest silence)
                    // once reads keep arriving after the load-time grace
                    // window; fully-buffered FMOD tracks stop reading at all.
                    streaming = false;
                    play_started = std::time::Instant::now();
                    log!(
                        "music bypass: playing {} ({} Hz, {}ch, {} KiB PCM, {} chunks)",
                        name,
                        sample_rate,
                        channels,
                        pcm.len() / 1024,
                        buffers.len()
                    );
                }
                Ok(PlayerCommand::Activity { name }) => {
                    if name == current_name && source != 0 {
                        // Reads still arriving well after the play started
                        // mean FMOD is genuinely streaming this track; those
                        // tracks may pause when reads dry up. Reads within the
                        // grace window are just the initial load burst.
                        if !streaming && play_started.elapsed() >= STREAM_GRACE {
                            streaming = true;
                        }
                        if streaming {
                            last_activity = std::time::Instant::now();
                        }
                        if paused {
                            al::alSourcePlay(source);
                            paused = false;
                            streaming = false;
                            play_started = std::time::Instant::now();
                            log!("music bypass: guest resumed; unpausing {}", name);
                        }
                    }
                }
                Ok(PlayerCommand::Restart { name }) => {
                    // FMOD seeks back to offset 0 right after opening a file
                    // (skipping metadata); that is not a track restart. Only
                    // honor rewinds of playback that has actually progressed.
                    let progressed = play_started.elapsed() >= STREAM_GRACE;
                    if name == current_name && source != 0 && !buffers.is_empty() && progressed {
                        al::alSourceStop(source);
                        let mut detach = 0;
                        al::alGetSourcei(source, AL_BUFFERS_QUEUED, &mut detach);
                        while detach > 0 {
                            let done = detach.min(64);
                            let mut scratch = vec![0u32; done as usize];
                            al::alSourceUnqueueBuffers(source, done, scratch.as_mut_ptr());
                            if al::alGetError() != aldef::AL_NO_ERROR {
                                break;
                            }
                            detach -= done;
                        }
                        al::alSourceQueueBuffers(
                            source,
                            buffers.len() as altypes::ALsizei,
                            buffers.as_ptr(),
                        );
                        al::alSourcePlay(source);
                        paused = false;
                        stopped = false;
                        streaming = false;
                        play_started = std::time::Instant::now();
                        log!("music bypass: restarted {} from the top", name);
                    }
                }
                Ok(PlayerCommand::Stop { name }) => {
                    if name == current_name && source != 0 {
                        al::alSourceStop(source);
                        paused = false;
                        stopped = true;
                        streaming = false;
                        log!("music bypass: guest stopped {}", name);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if source == 0 || buffers.is_empty() {
                        continue;
                    }
                    let mut state = 0;
                    let mut queued = 0;
                    let mut processed = 0;
                    al::alGetSourcei(source, aldef::AL_SOURCE_STATE, &mut state);
                    if state == AL_PLAYING
                        && streaming
                        && last_activity.elapsed() >= PAUSE_TIMEOUT
                    {
                        // Streaming tracks pause when reads dry up (GD pause
                        // menu); fully-buffered tracks keep looping forever.
                        al::alSourcePause(source);
                        paused = true;
                        log!("music bypass: guest went quiet; pausing {}", current_name);
                        continue;
                    }
                    if stopped || state != AL_STOPPED {
                        continue;
                    }
                    al::alGetSourcei(source, AL_BUFFERS_QUEUED, &mut queued);
                    al::alGetSourcei(source, AL_BUFFERS_PROCESSED, &mut processed);
                    if queued > 0 && processed == queued {
                        let mut drained = vec![0; queued as usize];
                        al::alSourceUnqueueBuffers(source, queued, drained.as_mut_ptr());
                        al::alSourceQueueBuffers(source, buffers.len() as altypes::ALsizei, buffers.as_ptr());
                        al::alSourcePlay(source);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        al::alSourceStop(source);
        al::alDeleteSources(1, &source);
        if !buffers.is_empty() {
            al::alDeleteBuffers(buffers.len() as altypes::ALsizei, buffers.as_ptr());
        }
        al::alcMakeContextCurrent(std::ptr::null_mut());
        al::alcDestroyContext(context);
        al::alcCloseDevice(device);
    }
}
