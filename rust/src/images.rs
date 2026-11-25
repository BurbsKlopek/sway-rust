use crate::catalog::http_agent;
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use image::imageops::FilterType;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImgKind {
    Poster,
    BackdropBlurred,
    BackdropCrisp,
}

pub struct DecodedImage {
    pub key: (String, ImgKind),
    pub img: ColorImage,
}

pub struct ImageCache {
    textures: HashMap<(String, ImgKind), TextureHandle>,
    order: VecDeque<(String, ImgKind)>,
    in_flight: HashSet<(String, ImgKind)>,
    failed: HashSet<(String, ImgKind)>,
    req_tx: Sender<(String, ImgKind, Context)>,
    res_rx: Receiver<Option<((String, ImgKind), ColorImage)>>,
}

impl ImageCache {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(String, ImgKind, Context)>();
        let (res_tx, res_rx) = mpsc::channel::<Option<((String, ImgKind), ColorImage)>>();

        // Spawn 4 lightweight worker threads sharing a bounded work queue via Mutex
        let shared_rx = std::sync::Arc::new(std::sync::Mutex::new(req_rx));
        for _ in 0..4 {
            let rx_c = std::sync::Arc::clone(&shared_rx);
            let tx_c = res_tx.clone();
            std::thread::spawn(move || {
                let agent = http_agent();
                loop {
                    let job = {
                        let Ok(guard) = rx_c.lock() else { break };
                        guard.recv()
                    };
                    let Ok((url, kind, ctx)) = job else { break };
                    let decoded = fetch_and_decode(&agent, &url, kind);
                    match decoded {
                        Some(img) => {
                            let _ = tx_c.send(Some(((url, kind), img)));
                        }
                        None => {
                            let _ = tx_c.send(Some((
                                (url, kind),
                                ColorImage::new([1, 1], egui::Color32::TRANSPARENT),
                            )));
                        }
                    }
                    ctx.request_repaint();
                }
            });
        }

        Self {
            textures: HashMap::new(),
            order: VecDeque::new(),
            in_flight: HashSet::new(),
            failed: HashSet::new(),
            req_tx,
            res_rx,
        }
    }

    pub fn poll(&mut self, ctx: &Context) {
        while let Ok(Some((key, img))) = self.res_rx.try_recv() {
            self.in_flight.remove(&key);
            if img.size == [1, 1] {
                self.failed.insert(key);
                continue;
            }
            let tex_name = format!("sway_img_{}_{:?}", key.0, key.1);
            let handle = ctx.load_texture(tex_name, img, TextureOptions::LINEAR);
            self.textures.insert(key.clone(), handle);
            self.order.push_back(key);

            // Cap GPU texture cache to 90 textures (~45 MB VRAM max)
            while self.order.len() > 90 {
                if let Some(oldest) = self.order.pop_front() {
                    self.textures.remove(&oldest);
                }
            }
        }
    }

    pub fn get_or_request(&mut self, url: &str, kind: ImgKind, ctx: &Context) -> Option<&TextureHandle> {
        if url.trim().is_empty() {
            return None;
        }
        let key = (url.to_string(), kind);
        if self.textures.contains_key(&key) {
            return self.textures.get(&key);
        }
        if !self.in_flight.contains(&key) && !self.failed.contains(&key) {
            self.in_flight.insert(key.clone());
            let _ = self.req_tx.send((key.0, kind, ctx.clone()));
        }
        None
    }

    pub fn prefetch(&mut self, url: &str, kind: ImgKind, ctx: &Context) {
        let _ = self.get_or_request(url, kind, ctx);
    }
}

fn fetch_and_decode(agent: &ureq::Agent, url: &str, kind: ImgKind) -> Option<ColorImage> {
    let resp = agent.get(url).call().ok()?;
    let mut bytes = Vec::with_capacity(262_144);
    resp.into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    let dyn_img = image::load_from_memory(&bytes).ok()?;

    match kind {
        ImgKind::Poster => {
            // Crisp 520x780 max poster decode for razor-sharp 4K cards with minimal VRAM
            let scaled = if dyn_img.width() > 540 || dyn_img.height() > 810 {
                dyn_img.resize(540, 810, FilterType::Triangle)
            } else {
                dyn_img
            };
            let rgba = scaled.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
        }
        ImgKind::BackdropCrisp => {
            let scaled = dyn_img.resize(960, 540, FilterType::Triangle);
            let rgba = scaled.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
        }
        ImgKind::BackdropBlurred => {
            // Downscale to 320x180 + 2-pass separable box blur -> ultra-creamy cinema aura at 0ms GPU cost!
            let small = dyn_img.resize_exact(320, 180, FilterType::Triangle);
            let mut rgba = small.to_rgba8();
            fast_box_blur_rgba(&mut rgba, 9);
            fast_box_blur_rgba(&mut rgba, 7);
            let size = [rgba.width() as usize, rgba.height() as usize];
            Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
        }
    }
}

fn fast_box_blur_rgba(img: &mut image::RgbaImage, radius: usize) {
    let w = img.width() as usize;
    let h = img.height() as usize;
    if w == 0 || h == 0 || radius == 0 {
        return;
    }
    let src = img.as_raw().clone();
    let mut tmp = vec![0u8; src.len()];
    let r = radius as isize;

    // Horizontal pass
    for y in 0..h {
        for x in 0..w {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut cnt = 0u32;
            let x0 = (x as isize - r).max(0) as usize;
            let x1 = (x as isize + r).min(w as isize - 1) as usize;
            for xx in x0..=x1 {
                let idx = (y * w + xx) * 4;
                sr += src[idx] as u32;
                sg += src[idx + 1] as u32;
                sb += src[idx + 2] as u32;
                cnt += 1;
            }
            let out_idx = (y * w + x) * 4;
            tmp[out_idx] = (sr / cnt) as u8;
            tmp[out_idx + 1] = (sg / cnt) as u8;
            tmp[out_idx + 2] = (sb / cnt) as u8;
            tmp[out_idx + 3] = 255;
        }
    }

    // Vertical pass
    let dst = img.as_mut();
    for y in 0..h {
        let y0 = (y as isize - r).max(0) as usize;
        let y1 = (y as isize + r).min(h as isize - 1) as usize;
        let cnt = (y1 - y0 + 1) as u32;
        for x in 0..w {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            for yy in y0..=y1 {
                let idx = (yy * w + x) * 4;
                sr += tmp[idx] as u32;
                sg += tmp[idx + 1] as u32;
                sb += tmp[idx + 2] as u32;
            }
            let out_idx = (y * w + x) * 4;
            dst[out_idx] = (sr / cnt) as u8;
            dst[out_idx + 1] = (sg / cnt) as u8;
            dst[out_idx + 2] = (sb / cnt) as u8;
            dst[out_idx + 3] = 255;
        }
    }
}
