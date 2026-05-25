pub struct GlyphAtlas {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    uvs: Vec<([f32; 2], [f32; 2])>,
}

impl GlyphAtlas {
    pub fn build(font_bytes: &[u8], px_size: f32) -> Self {
        let font = fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
            .expect("fontdue: invalid font bytes");

        let (m_metrics, _) = font.rasterize('M', px_size);
        let cell_w = m_metrics.advance_width.ceil() as u32;
        let cell_h = cell_w * 2;

        // 16×6 atlas grid for printable ASCII 0x20..=0x7E (95 chars)
        let atlas_cols = 16u32;
        let atlas_rows = 6u32;
        let atlas_w = atlas_cols * cell_w;
        let atlas_h = atlas_rows * cell_h;
        let mut data = vec![0u8; (atlas_w * atlas_h) as usize];
        let mut uvs = vec![([0f32; 2], [0f32; 2]); 256];

        for code in 0x20u32..=0x7Eu32 {
            let ch = char::from_u32(code).unwrap();
            let slot = code - 0x20;
            let gx = (slot % atlas_cols) * cell_w;
            let gy = (slot / atlas_cols) * cell_h;

            let (m, bitmap) = font.rasterize(ch, px_size);
            if !bitmap.is_empty() {
                let y_off = cell_h.saturating_sub(m.height as u32);
                for row in 0..m.height {
                    for col in 0..m.width {
                        let dst_x = gx + col as u32;
                        let dst_y = gy + y_off + row as u32;
                        if dst_x < atlas_w && dst_y < atlas_h {
                            data[(dst_y * atlas_w + dst_x) as usize] = bitmap[row * m.width + col];
                        }
                    }
                }
            }

            uvs[code as usize] = (
                [gx as f32 / atlas_w as f32, gy as f32 / atlas_h as f32],
                [(gx + cell_w) as f32 / atlas_w as f32, (gy + cell_h) as f32 / atlas_h as f32],
            );
        }

        // Space glyph (0x20) is already blank — transparent background shows cell bg.
        // Reuse space UV for unknown chars.
        let space_uv = uvs[0x20];
        for code in 0u32..0x20 {
            uvs[code as usize] = space_uv;
        }
        for code in 0x7Fu32..256u32 {
            uvs[code as usize] = space_uv;
        }

        GlyphAtlas { data, width: atlas_w, height: atlas_h, cell_w, cell_h, uvs }
    }

    pub fn uv_for_char(&self, ch: char) -> ([f32; 2], [f32; 2]) {
        let idx = ch as u32;
        if idx < self.uvs.len() as u32 {
            self.uvs[idx as usize]
        } else {
            self.uvs[0x20]
        }
    }
}
