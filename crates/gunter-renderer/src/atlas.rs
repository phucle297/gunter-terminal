use std::collections::HashMap;

const STATIC_COLS: u32 = 16;
const STATIC_ROWS: u32 = 16;
const DYN_NARROW_ROWS: u32 = 8;
const DYN_NARROW_SLOTS: usize = (STATIC_COLS * DYN_NARROW_ROWS) as usize; // 128
const DYN_WIDE_COLS: u32 = 8;
const DYN_WIDE_ROWS: u32 = 4;
const DYN_WIDE_SLOTS: usize = (DYN_WIDE_COLS * DYN_WIDE_ROWS) as usize; // 32

pub struct GlyphAtlas {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub ascender: f32,
    uvs: HashMap<u32, ([f32; 2], [f32; 2])>,
    pub space_uv: ([f32; 2], [f32; 2]),
    font: fontdue::Font,
    px_size: f32,
    dyn_narrow_uvs: HashMap<char, ([f32; 2], [f32; 2])>,
    dyn_narrow_chars: [Option<char>; DYN_NARROW_SLOTS],
    dyn_narrow_next: u32,
    dyn_wide_uvs: HashMap<char, ([f32; 2], [f32; 2])>,
    dyn_wide_chars: [Option<char>; DYN_WIDE_SLOTS],
    dyn_wide_next: u32,
    pub dynamic_dirty: bool,
    pub dyn_pixel_y: u32,
}

impl GlyphAtlas {
    pub fn build(font_bytes: &[u8], px_size: f32) -> Self {
        let font = fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
            .expect("fontdue: invalid font bytes");

        let (m_metrics, _) = font.rasterize('M', px_size);
        let cell_w = m_metrics.advance_width.ceil() as u32;

        let line_metrics = font.horizontal_line_metrics(px_size)
            .expect("font missing horizontal line metrics");
        let ascender = line_metrics.ascent;
        let descender = line_metrics.descent;
        let cell_h = (ascender + descender.abs()).ceil() as u32;

        let atlas_w = STATIC_COLS * cell_w;
        let static_h = STATIC_ROWS * cell_h;
        let dyn_narrow_h = DYN_NARROW_ROWS * cell_h;
        let dyn_wide_h = DYN_WIDE_ROWS * cell_h;
        let atlas_h = static_h + dyn_narrow_h + dyn_wide_h;

        let mut data = vec![0u8; (atlas_w * atlas_h) as usize];
        let mut uvs: HashMap<u32, ([f32; 2], [f32; 2])> = HashMap::new();

        // --- ASCII 0x20-0x7E → static slots 0-94 ---
        for code in 0x20u32..=0x7Eu32 {
            let ch = char::from_u32(code).unwrap();
            let slot = code - 0x20;
            let gx = (slot % STATIC_COLS) * cell_w;
            let gy = (slot / STATIC_COLS) * cell_h;
            blit_glyph(&mut data, &font, ch, gx, gy, atlas_w, atlas_h, cell_w, cell_h, ascender, px_size);
            uvs.insert(code, slot_uv(gx, gy, cell_w, cell_h, atlas_w, atlas_h));
        }

        // --- Box-drawing U+2500-U+257F (128 chars) → slots 96-223 ---
        for code in 0x2500u32..=0x257Fu32 {
            let ch = char::from_u32(code).unwrap();
            let slot = 96 + (code - 0x2500);
            let gx = (slot % STATIC_COLS) * cell_w;
            let gy = (slot / STATIC_COLS) * cell_h;
            blit_glyph(&mut data, &font, ch, gx, gy, atlas_w, atlas_h, cell_w, cell_h, ascender, px_size);
            uvs.insert(code, slot_uv(gx, gy, cell_w, cell_h, atlas_w, atlas_h));
        }

        // --- Block elements U+2580-U+259F (32 chars) → slots 224-255 ---
        for code in 0x2580u32..=0x259Fu32 {
            let ch = char::from_u32(code).unwrap();
            let slot = 224 + (code - 0x2580);
            let gx = (slot % STATIC_COLS) * cell_w;
            let gy = (slot / STATIC_COLS) * cell_h;
            blit_glyph(&mut data, &font, ch, gx, gy, atlas_w, atlas_h, cell_w, cell_h, ascender, px_size);
            uvs.insert(code, slot_uv(gx, gy, cell_w, cell_h, atlas_w, atlas_h));
        }

        let space_uv = *uvs.get(&0x20).unwrap_or(&([0.0; 2], [0.0; 2]));

        GlyphAtlas {
            data,
            width: atlas_w,
            height: atlas_h,
            cell_w,
            cell_h,
            ascender,
            uvs,
            space_uv,
            font,
            px_size,
            dyn_narrow_uvs: HashMap::new(),
            dyn_narrow_chars: [None; DYN_NARROW_SLOTS],
            dyn_narrow_next: 0,
            dyn_wide_uvs: HashMap::new(),
            dyn_wide_chars: [None; DYN_WIDE_SLOTS],
            dyn_wide_next: 0,
            dynamic_dirty: false,
            dyn_pixel_y: static_h,
        }
    }

    pub fn uv_for_char(&mut self, ch: char) -> ([f32; 2], [f32; 2]) {
        let code = ch as u32;
        if let Some(&uv) = self.uvs.get(&code) {
            return uv;
        }
        if let Some(&uv) = self.dyn_narrow_uvs.get(&ch) {
            return uv;
        }
        self.cache_narrow(ch)
    }

    pub fn uv_for_wide_char(&mut self, ch: char) -> ([f32; 2], [f32; 2]) {
        if let Some(&uv) = self.dyn_wide_uvs.get(&ch) {
            return uv;
        }
        self.cache_wide(ch)
    }

    fn cache_narrow(&mut self, ch: char) -> ([f32; 2], [f32; 2]) {
        let (m, bitmap) = self.font.rasterize(ch, self.px_size);

        let slot = self.dyn_narrow_next;
        self.dyn_narrow_next = (slot + 1) % DYN_NARROW_SLOTS as u32;

        if let Some(old_ch) = self.dyn_narrow_chars[slot as usize] {
            self.dyn_narrow_uvs.remove(&old_ch);
        }
        self.dyn_narrow_chars[slot as usize] = Some(ch);

        let col = slot % STATIC_COLS;
        let row = slot / STATIC_COLS;
        let gx = col * self.cell_w;
        let gy = (STATIC_ROWS + row) * self.cell_h;

        clear_slot(&mut self.data, gx, gy, self.cell_w, self.cell_h, self.width);
        blit_metrics(&mut self.data, &m, &bitmap, gx, gy, self.width, self.height, self.cell_w, self.cell_h, self.ascender);

        let uv = slot_uv(gx, gy, self.cell_w, self.cell_h, self.width, self.height);
        self.dyn_narrow_uvs.insert(ch, uv);
        self.dynamic_dirty = true;
        uv
    }

    fn cache_wide(&mut self, ch: char) -> ([f32; 2], [f32; 2]) {
        let wide_cell_w = self.cell_w * 2;
        let (m, bitmap) = self.font.rasterize(ch, self.px_size);

        let slot = self.dyn_wide_next;
        self.dyn_wide_next = (slot + 1) % DYN_WIDE_SLOTS as u32;

        if let Some(old_ch) = self.dyn_wide_chars[slot as usize] {
            self.dyn_wide_uvs.remove(&old_ch);
        }
        self.dyn_wide_chars[slot as usize] = Some(ch);

        let col = slot % DYN_WIDE_COLS;
        let row = slot / DYN_WIDE_COLS;
        let gx = col * wide_cell_w;
        let gy = (STATIC_ROWS + DYN_NARROW_ROWS + row) * self.cell_h;

        clear_slot(&mut self.data, gx, gy, wide_cell_w, self.cell_h, self.width);
        blit_metrics(&mut self.data, &m, &bitmap, gx, gy, self.width, self.height, wide_cell_w, self.cell_h, self.ascender);

        let uv = slot_uv(gx, gy, wide_cell_w, self.cell_h, self.width, self.height);
        self.dyn_wide_uvs.insert(ch, uv);
        self.dynamic_dirty = true;
        uv
    }
}

fn slot_uv(gx: u32, gy: u32, w: u32, h: u32, atlas_w: u32, atlas_h: u32) -> ([f32; 2], [f32; 2]) {
    (
        [gx as f32 / atlas_w as f32, gy as f32 / atlas_h as f32],
        [(gx + w) as f32 / atlas_w as f32, (gy + h) as f32 / atlas_h as f32],
    )
}

fn clear_slot(data: &mut [u8], gx: u32, gy: u32, w: u32, h: u32, atlas_w: u32) {
    for r in 0..h {
        let row_start = ((gy + r) * atlas_w + gx) as usize;
        for c in 0..(w as usize) {
            if row_start + c < data.len() {
                data[row_start + c] = 0;
            }
        }
    }
}

fn blit_glyph(
    data: &mut Vec<u8>,
    font: &fontdue::Font,
    ch: char,
    gx: u32, gy: u32,
    atlas_w: u32, atlas_h: u32,
    cell_w: u32, cell_h: u32,
    ascender: f32,
    px_size: f32,
) {
    let (m, bitmap) = font.rasterize(ch, px_size);
    blit_metrics(data, &m, &bitmap, gx, gy, atlas_w, atlas_h, cell_w, cell_h, ascender);
}

fn blit_metrics(
    data: &mut [u8],
    m: &fontdue::Metrics,
    bitmap: &[u8],
    gx: u32, gy: u32,
    atlas_w: u32, atlas_h: u32,
    _cell_w: u32, cell_h: u32,
    ascender: f32,
) {
    if bitmap.is_empty() { return; }
    let ymax = m.ymin as f32 + m.height as f32;
    let y_off = (ascender - ymax).max(0.0) as u32;
    for row in 0..m.height {
        for col in 0..m.width {
            let x_off = m.xmin.max(0) as u32;
            let dst_x = gx + x_off + col as u32;
            let dst_y = gy + y_off + row as u32;
            if dst_x < atlas_w && dst_y < atlas_h && dst_y < gy + cell_h {
                data[(dst_y * atlas_w + dst_x) as usize] = bitmap[row * m.width + col];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_font() -> Option<Vec<u8>> {
        let candidates = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
            "/usr/share/fonts/noto/NotoMono-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        ];
        candidates.iter().find_map(|p| std::fs::read(p).ok())
    }

    #[test]
    fn cell_h_uses_font_metrics_not_heuristic() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let atlas = GlyphAtlas::build(&font_bytes, 14.0);
        assert!(atlas.ascender > 0.0);
        assert!(atlas.cell_h > 0);
    }

    #[test]
    fn baseline_align_does_not_panic() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let atlas = GlyphAtlas::build(&font_bytes, 14.0);
        assert!(!atlas.data.is_empty());
    }

    #[test]
    fn left_bearing_applied_no_panic() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let atlas = GlyphAtlas::build(&font_bytes, 14.0);
        assert!(atlas.data.iter().any(|&b| b > 0));
    }

    #[test]
    fn box_drawing_char_has_distinct_uv() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let mut atlas = GlyphAtlas::build(&font_bytes, 14.0);
        let space_uv = atlas.uv_for_char(' ');
        let box_uv = atlas.uv_for_char('─'); // U+2500
        assert_ne!(box_uv, space_uv, "box-drawing char should have distinct UV from space");
    }

    #[test]
    fn block_element_char_has_distinct_uv() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let mut atlas = GlyphAtlas::build(&font_bytes, 14.0);
        let space_uv = atlas.uv_for_char(' ');
        let block_uv = atlas.uv_for_char('▀'); // U+2580
        assert_ne!(block_uv, space_uv, "block element char should have distinct UV from space");
    }

    #[test]
    fn dynamic_cache_rasterizes_unknown_char() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let mut atlas = GlyphAtlas::build(&font_bytes, 14.0);
        let space_uv = atlas.space_uv;
        let uv = atlas.uv_for_char('é'); // not in static atlas
        assert_ne!(uv, space_uv, "dynamic char should get a unique UV slot");
    }

    #[test]
    fn dynamic_cache_marks_dirty() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let mut atlas = GlyphAtlas::build(&font_bytes, 14.0);
        assert!(!atlas.dynamic_dirty, "should start clean");
        atlas.uv_for_char('ñ'); // cache miss → rasterize
        assert!(atlas.dynamic_dirty, "should be dirty after cache miss");
    }

    #[test]
    fn dynamic_cache_hit_does_not_re_dirty() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let mut atlas = GlyphAtlas::build(&font_bytes, 14.0);
        atlas.uv_for_char('ñ'); // first miss
        atlas.dynamic_dirty = false;
        atlas.uv_for_char('ñ'); // cache hit — should NOT set dirty
        assert!(!atlas.dynamic_dirty, "cache hit should not mark dirty");
    }

    #[test]
    fn wide_char_gets_wide_uv_slot() {
        let font_bytes = match load_font() { Some(b) => b, None => return };
        let mut atlas = GlyphAtlas::build(&font_bytes, 14.0);
        let narrow_uv = atlas.uv_for_char('A');
        let narrow_width = narrow_uv.1[0] - narrow_uv.0[0];
        let wide_uv = atlas.uv_for_wide_char('中'); // CJK wide char
        let wide_width = wide_uv.1[0] - wide_uv.0[0];
        // Wide UV should span ~2x the U range of a narrow UV
        assert!(wide_width > narrow_width * 1.5, "wide UV should span ~2x narrow UV width");
    }
}
