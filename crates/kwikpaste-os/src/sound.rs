//! 复制成功提示音（1.x 的 `clipboard/sound.rs`）。10 KB 的单声道 16-bit PCM WAV 直接打进二进制。
//!
//! - Windows：系统 `PlaySoundW` 异步播放，与 1.x 相同。
//! - macOS：AppKit 的 `NSSound`。1.x 用 rodio，它经 cpal 带进 coreaudio-sys（构建时要 bindgen 和
//!   libclang）；`NSSound` 是已经链接的系统 API，和 Windows 一侧对称。每次播放开一条短命线程，
//!   持有 `NSSound` 直到放完。

const COPY_SOUND: &[u8] = include_bytes!("../assets/sounds/copy.wav");

/// 异步播放一次复制提示音，不阻塞调用方；失败只记日志。
#[cfg(target_os = "windows")]
pub fn play_copy() {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;

    // SND_MEMORY：第一个参数指向内存里的整段 WAV；SND_ASYNC 要求它在播放期间一直有效，静态字节满足。
    let played = unsafe {
        PlaySoundW(
            PCWSTR(COPY_SOUND.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        )
    };
    if played.as_bool() {
        log::debug!("copy sound started");
    } else {
        log::warn!("the copy sound could not be played");
    }
}

/// 异步播放一次复制提示音，不阻塞调用方；失败只记日志。
#[cfg(target_os = "macos")]
pub fn play_copy() {
    let spawned = std::thread::Builder::new()
        .name("copy-sound".to_owned())
        .spawn(|| {
            if let Err(err) = mac::play_until_done(COPY_SOUND) {
                log::warn!("the copy sound could not be played: {err}");
            }
        });
    if let Err(err) = spawned {
        log::warn!("the copy sound thread could not start: {err}");
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::{Duration, Instant};

    use objc2::AllocAnyThread as _;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSData;

    /// 播放超过这个时长就不再等（提示音本身不到半秒）。
    const MAX_WAIT: Duration = Duration::from_secs(5);

    pub(super) fn decode(bytes: &[u8]) -> Option<Retained<NSSound>> {
        let data = NSData::with_bytes(bytes);
        NSSound::initWithData(NSSound::alloc(), &data)
    }

    /// 解码并播放，阻塞到放完。
    pub(super) fn play_until_done(bytes: &[u8]) -> Result<(), &'static str> {
        autoreleasepool(|_| {
            let sound = decode(bytes).ok_or("the sound data could not be decoded")?;
            if !sound.play() {
                return Err("NSSound refused to play");
            }
            let deadline = Instant::now() + MAX_WAIT;
            while sound.isPlaying() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_sound_is_a_pcm_wave_file() {
        assert_eq!(&COPY_SOUND[..4], b"RIFF");
        assert_eq!(&COPY_SOUND[8..12], b"WAVE");
    }

    /// 只解码不播放：CI 的 macOS 机器上验证 NSSound 认得这段 WAV。
    #[cfg(target_os = "macos")]
    #[test]
    fn nssound_decodes_the_copy_sound() {
        let sound = mac::decode(COPY_SOUND).expect("NSSound decodes copy.wav");

        assert!(sound.duration() > 0.0);
    }
}
