use std::collections::HashMap;

use font_kit::{
    font::Font,
    metrics::Metrics,
    properties::{Properties, Stretch, Style, Weight},
    source::SystemSource,
};
use lru::LruCache;
use skribo::{FontCollection, FontFamily, FontRef as SkriboFont, LayoutSession, TextStyle};
use skulpin::skia_safe::{Data, Font as SkiaFont, TextBlob, TextBlobBuilder, Typeface};

use log::{trace, warn};
use cfg_if::cfg_if as define;

const STANDARD_CHARACTER_STRING: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";

define! {
    if #[cfg(target_os = "windows")] {
        const SYSTEM_DEFAULT_FONT: &str = "Consolas";
        const SYSTEM_SYMBOL_FONT: &str = "Segoe UI Symbol";
        const SYSTEM_EMOJI_FONT: &str = "Segoe UI Emoji";
    } else if #[cfg(target_os = "linux")] {
        const SYSTEM_DEFAULT_FONT: &str = "Droid Sans Mono";
        const SYSTEM_SYMBOL_FONT: &str = "Unifont";
        const SYSTEM_EMOJI_FONT: &str = "Noto Color Emoji";
    } else if #[cfg(target_os = "macos")] {
        const SYSTEM_DEFAULT_FONT: &str = "Menlo";
        const SYSTEM_SYMBOL_FONT: &str = "Apple Symbols";
        const SYSTEM_EMOJI_FONT: &str = "Apple Color Emoji";
    }
}

const EXTRA_SYMBOL_FONT: &str = "Extra Symbols.otf";
const MISSING_GLYPH_FONT: &str = "Missing Glyphs.otf";

#[cfg(feature = "embed-fonts")]
#[derive(RustEmbed)]
#[folder = "assets/fonts/"]
struct Asset;

const DEFAULT_FONT_SIZE: f32 = 14.0;

#[derive(Clone)]
pub struct ExtendedFontFamily {
    pub fonts: Vec<SkriboFont>
}

impl ExtendedFontFamily {
    pub fn new() -> ExtendedFontFamily {
        ExtendedFontFamily { fonts: Vec::new() }
    }

    pub fn add_font(&mut self, font: SkriboFont) {
        self.fonts.push(font);
    }

    pub fn get(&self, props: Properties) -> Option<&Font> {
        for handle in &self.fonts {
            let font = &handle.font;
            let properties = font.properties();

            if properties.weight == props.weight && properties.style == props.style {
                return Some(&font);
            }
        }

        None
    }

    pub fn to_family(self) -> FontFamily {
        let mut new_family = FontFamily::new();
    
        for font in self.fonts {
            new_family.add_font(font);
        }
    
        new_family
    }
}

pub struct FontLoader {
    cache: LruCache<String, ExtendedFontFamily>,
    source: SystemSource
}

impl FontLoader {
    pub fn new() -> FontLoader {
        FontLoader {
            cache: LruCache::new(10),
            source: SystemSource::new()
        }
    }

    fn get(&mut self, font_name: &str) -> Option<ExtendedFontFamily> {
        if let Some(family) = self.cache.get(&String::from(font_name)) {
            return Some(family.clone());
        }

        None
    }

    #[cfg(feature = "embed-fonts")]
    fn load_from_asset(&mut self, font_name: &str) -> Option<ExtendedFontFamily> {
        let mut family = ExtendedFontFamily::new();

        Asset::get(font_name)
            .and_then(|font_data| Font::from_bytes(font_data.to_vec().into(), 0).ok())
            .map(|font| family.add_font(SkriboFont::new(font)));

        self.cache.put(String::from(font_name), family);
        self.get(font_name)
    }

    fn load(&mut self, font_name: &str) -> Option<ExtendedFontFamily> {
        let handle = match self.source.select_family_by_name(font_name) {
            Ok(it) => it,
            _ => return None,
        };

        let fonts = handle.fonts();
        let mut family = ExtendedFontFamily::new();

        for font in fonts.into_iter() {
            if let Ok(font) = font.load() {
                family.add_font(SkriboFont::new(font));
            }
        }

        self.cache.put(String::from(font_name), family);
        self.get(font_name)
    }

    pub fn get_or_load(&mut self, font_name: &str, asset: bool) -> Option<ExtendedFontFamily> {
        if let Some(family) = self.get(font_name) {
            return Some(family)
        }

        if asset { self.load_from_asset(font_name) } else { self.load(font_name) }
    }
}

#[derive(new, Clone, Hash, PartialEq, Eq, Debug)]
struct ShapeKey {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

pub fn build_collection_by_font_name(
    loader: &mut FontLoader,
    font_name: Option<&str>,
    bold: bool,
    italic: bool,
) -> FontCollection {
    let mut collection = FontCollection::new();

    if let Some(font_name) = font_name {
        if let Some(family) = loader.get_or_load(font_name, false) {
            let weight = if bold { Weight::BOLD } else { Weight::NORMAL };
            let style = if italic { Style::Italic } else { Style::Normal };
            let properties = Properties {
                weight,
                style,
                stretch: Stretch::NORMAL,
            };

            if let Some(font) = family.get(properties) {
                collection.add_family(FontFamily::new_from_font(font.clone()));
            }
        }
    }

    if let Some(family) = loader.get_or_load(SYSTEM_SYMBOL_FONT, false) {
        collection.add_family(family.to_family());
    }

    if let Some(family) = loader.get_or_load(SYSTEM_EMOJI_FONT, false) {
        collection.add_family(family.to_family());
    }

    if let Some(family) = loader.get_or_load(EXTRA_SYMBOL_FONT, true) {
        collection.add_family(family.to_family());
    }

    if let Some(family) = loader.get_or_load(MISSING_GLYPH_FONT, true) {
        collection.add_family(family.to_family());
    }

    collection
}

struct FontSet {
    normal: FontCollection,
    bold: FontCollection,
    italic: FontCollection,
}

impl FontSet {
    fn new(font_name: Option<&str>, mut loader: &mut FontLoader) -> FontSet {
        FontSet {
            normal: build_collection_by_font_name(&mut loader, font_name, false, false),
            bold: build_collection_by_font_name(&mut loader, font_name, true, false),
            italic: build_collection_by_font_name(&mut loader, font_name, false, true),
        }
    }

    fn get(&self, bold: bool, italic: bool) -> &FontCollection {
        match (bold, italic) {
            (true, _) => &self.bold,
            (false, false) => &self.normal,
            (false, true) => &self.italic,
        }
    }
}

pub struct CachingShaper {
    pub font_name: Option<String>,
    pub base_size: f32,
    font_set: FontSet,
    font_loader: FontLoader,
    font_cache: LruCache<String, SkiaFont>,
    blob_cache: LruCache<ShapeKey, Vec<TextBlob>>,
}

fn build_skia_font_from_skribo_font(skribo_font: &SkriboFont, base_size: f32) -> Option<SkiaFont> {
    let font_data = skribo_font.font.copy_font_data()?;
    let skia_data = Data::new_copy(&font_data[..]);
    let typeface = Typeface::from_data(skia_data, None)?;

    Some(SkiaFont::from_typeface(typeface, base_size))
}

impl CachingShaper {
    pub fn new() -> CachingShaper {
        let mut loader = FontLoader::new();

        CachingShaper {
            font_name: Some(String::from(SYSTEM_DEFAULT_FONT)),
            base_size: DEFAULT_FONT_SIZE,
            font_set: FontSet::new(Some(SYSTEM_DEFAULT_FONT), &mut loader),
            font_loader: loader,
            font_cache: LruCache::new(10),
            blob_cache: LruCache::new(10000),
        }
    }

    fn get_skia_font(&mut self, skribo_font: &SkriboFont) -> Option<&SkiaFont> {
        let font_name = skribo_font.font.postscript_name()?;

        if !self.font_cache.contains(&font_name) {
            let font = build_skia_font_from_skribo_font(skribo_font, self.base_size)?;
            self.font_cache.put(font_name.clone(), font);
        }

        self.font_cache.get(&font_name)
    }

    fn metrics(&self) -> Metrics {
        self.font_set
            .normal
            .itemize("a")
            .next()
            .unwrap()
            .1
            .font
            .metrics()
    }

    pub fn shape(&mut self, text: &str, bold: bool, italic: bool) -> Vec<TextBlob> {
        let style = TextStyle {
            size: self.base_size,
        };

        let session = LayoutSession::create(text, &style, &self.font_set.get(bold, italic));
        let metrics = self.metrics();
        let ascent = metrics.ascent * self.base_size / metrics.units_per_em as f32;
        let mut blobs = Vec::new();

        for layout_run in session.iter_all() {
            let skribo_font = layout_run.font();

            if let Some(skia_font) = self.get_skia_font(&skribo_font) {
                let mut blob_builder = TextBlobBuilder::new();
                let count = layout_run.glyphs().count();
                let (glyphs, positions) =
                    blob_builder.alloc_run_pos_h(&skia_font, count, ascent, None);

                for (i, glyph) in layout_run.glyphs().enumerate() {
                    glyphs[i] = glyph.glyph_id as u16;
                    positions[i] = glyph.offset.x;
                }

                blobs.push(blob_builder.make().unwrap());
            } else {
                warn!("Could not load scribo font");
            }
        }

        blobs
    }

    pub fn shape_cached(&mut self, text: &str, bold: bool, italic: bool) -> &Vec<TextBlob> {
        let key = ShapeKey::new(text.to_string(), bold, italic);

        if !self.blob_cache.contains(&key) {
            let blobs = self.shape(text, bold, italic);
            self.blob_cache.put(key.clone(), blobs);
        }

        self.blob_cache.get(&key).unwrap()
    }

    pub fn change_font(&mut self, font_name: Option<&str>, base_size: Option<f32>) {
        trace!("Font changed {:?} {:?}", &font_name, &base_size);
        self.font_name = font_name.map(|name| name.to_string());
        self.base_size = base_size.unwrap_or(DEFAULT_FONT_SIZE);
        self.font_set = FontSet::new(font_name, &mut self.font_loader);
        self.font_cache.clear();
        self.blob_cache.clear();
    }

    pub fn font_base_dimensions(&mut self) -> (f32, f32) {
        let metrics = self.metrics();
        let font_height = (metrics.ascent - metrics.descent) * self.base_size / metrics.units_per_em as f32;
        let style = TextStyle { size: self.base_size, };
        let session = LayoutSession::create(STANDARD_CHARACTER_STRING, &style, &self.font_set.normal);
        let layout_run = session.iter_all().next().unwrap();
        let glyph_offsets: Vec<f32> = layout_run.glyphs().map(|glyph| glyph.offset.x).collect();
        let glyph_advances: Vec<f32> = glyph_offsets
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect();

        let mut amounts = HashMap::new();

        for advance in glyph_advances.iter() {
            amounts
                .entry(advance.to_string())
                .and_modify(|e| *e += 1)
                .or_insert(1);
        }

        let (font_width, _) = amounts.into_iter().max_by_key(|(_, count)| *count).unwrap();
        let font_width = font_width.parse::<f32>().unwrap();

        (font_width, font_height)
    }

    pub fn underline_position(&mut self) -> f32 {
        let metrics = self.metrics();
        -metrics.underline_position * self.base_size / metrics.units_per_em as f32
    }
}
