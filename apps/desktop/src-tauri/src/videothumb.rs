//! #16: first-frame thumbnails for videos.
//!
//! The decoder decision (DESIGN D18): no vendored FFmpeg and no pure-Rust
//! decoder gamble. Windows itself ships decoders for everything this library
//! meets (H.264/HEVC MP4, VP8/VP9 WebM), so we ask the Shell's thumbnail
//! provider (Media Foundation) — the exact path Explorer uses — and copy the
//! returned bitmap into RGBA. Best effort: no codec, no thumbnail; the item
//! still stores and plays. Remote/vault videos get written to a temp file
//! first (the Shell handler takes a path).

/// RGBA pixels of the first frame, row order top-down.
pub struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn first_frame(path: &std::path::Path) -> Option<Frame> {
    #[cfg(windows)]
    {
        win::first_frame(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        BHID_ThumbnailHandler, IShellItem, IThumbnailProvider, SHCreateItemFromParsingName,
        WTS_ALPHATYPE,
    };

    use super::Frame;

    /// Uninitialize COM only if we initialized it on this thread.
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() }
            }
        }
    }

    pub fn first_frame(path: &Path) -> Option<Frame> {
        // If the thread already has COM in another model we just don't own
        // teardown; any harder failure means no thumbnail.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let owned = hr.is_ok();
        if !owned && hr.0 != -2147417823 {
            // -2147417823 == RPC_E_CHANGED_MODE (0x80010106)
            return None;
        }
        let _guard = ComGuard(owned);
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe { inner(&wide) }
    }

    unsafe fn inner(wide: &[u16]) -> Option<Frame> {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let provider: IThumbnailProvider = item.BindToHandler(None, &BHID_ThumbnailHandler).ok()?;

        let mut hbmp = HBITMAP::default();
        let mut alpha = WTS_ALPHATYPE(0);
        provider.GetThumbnail(360, &mut hbmp, &mut alpha).ok()?;
        if hbmp.0.is_null() {
            return None;
        }

        let mut bm = BITMAP {
            bmType: 0,
            bmWidth: 0,
            bmHeight: 0,
            bmWidthBytes: 0,
            bmPlanes: 0,
            bmBitsPixel: 0,
            bmBits: std::ptr::null_mut(),
        };
        if GetObjectW(
            HGDIOBJ(hbmp.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        ) == 0
        {
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            return None;
        }
        let w = bm.bmWidth.max(0) as u32;
        let h = bm.bmHeight.max(0) as u32;
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            return None;
        }

        // Copy pixels ourselves: force 32bpp BGRA top-down, then flip to RGBA.
        let hdc = GetDC(None);
        let mut out = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // negative = top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        let lines = GetDIBits(
            hdc,
            hbmp,
            0,
            h,
            Some(buf.as_mut_ptr().cast()),
            &mut out,
            DIB_RGB_COLORS,
        );
        let _ = ReleaseDC(None, hdc);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        if lines == 0 {
            return None;
        }

        let mut rgba = buf;
        for p in rgba.as_chunks_mut::<4>().0 {
            p.swap(0, 2); // BGR → RGB
            p[3] = 255; // video frames carry no alpha
        }
        Some(Frame {
            rgba,
            width: w,
            height: h,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not automated — needs a real video file. Run by hand against any mp4:
    /// `GOON_FF_PATH=D:\some.mp4 cargo test -p goonflow-desktop --lib -- --ignored --nocapture ff_live`
    ///
    /// Missing input here means **skip**, not fail: unlike the f95 probes (where a
    /// silent skip is exactly how #46 hid for a release), nothing is being claimed
    /// about the product when there is no video on this machine — only that one of
    /// five live probes cannot run. Keeping it a skip is what makes "run every
    /// `#[ignore]`d probe in one command" a usable check.
    #[test]
    #[ignore]
    fn first_frame_live_probe() {
        let Ok(p) = std::env::var("GOON_FF_PATH") else {
            eprintln!(
                "GOON_FF_PATH not set - no video file to grab a frame from, \
                 so this probe is skipped (not a failure)"
            );
            return;
        };
        let f = first_frame(std::path::Path::new(&p)).expect("no frame extracted");
        println!(
            "frame {}x{}, {} bytes RGBA",
            f.width,
            f.height,
            f.rgba.len()
        );
        assert!(f.width > 0 && f.height > 0);
    }
}
