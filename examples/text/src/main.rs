extern crate pdf;

use std::env::args;
use std::collections::HashMap;
use std::convert::TryInto;

use pdf::file::File;
use pdf::content::*;
use pdf::primitive::Primitive;
use pdf::font::*;
use pdf::parser::Lexer;
use pdf::parser::parse_with_lexer;
use pdf::object::{Resolve, NoResolve, RcRef};
use pdf::encoding::BaseEncoding;
use pdf::error::PdfError;

use byteorder::BE;
use utf16_ext::Utf16ReadExt;

fn utf16be_to_string(mut data: &[u8]) -> String {
    (&mut data).utf16_chars::<BE>().map(|c| c.unwrap()).collect()
}

// totally not a steaming pile of hacks
fn parse_cmap(data: &[u8]) -> HashMap<u16, String> {
    println!("{}", std::str::from_utf8(data).unwrap());
    let mut lexer = Lexer::new(data);
    let mut map = HashMap::new();
    while let Ok(substr) = lexer.next() {
        match substr.as_slice() {
            b"beginbfchar" => loop {
                let a = parse_with_lexer(&mut lexer, &NoResolve);
                let b = parse_with_lexer(&mut lexer, &NoResolve);
                match (a, b) {
                    (Ok(Primitive::String(cid_data)), Ok(Primitive::String(unicode_data))) => {
                        let cid = u16::from_be_bytes(cid_data.as_bytes().try_into().unwrap());
                        let unicode = utf16be_to_string(unicode_data.as_bytes());
                        map.insert(cid, unicode);
                    }
                    _ => break
                }
            }
            b"beginbfrange" => loop {
                let a = parse_with_lexer(&mut lexer, &NoResolve);
                let b = parse_with_lexer(&mut lexer, &NoResolve);
                let c = parse_with_lexer(&mut lexer, &NoResolve);
                match (a, b, c) {
                    (Ok(Primitive::String(cid_start_data)), Ok(Primitive::String(cid_end_data)), Ok(Primitive::String(unicode_data))) => {
                        let cid_start = u16::from_be_bytes(cid_start_data.as_bytes().try_into().unwrap());
                        let cid_end = u16::from_be_bytes(cid_end_data.as_bytes().try_into().unwrap());
                        let mut unicode_data = unicode_data.into_bytes();

                        for cid in cid_start ..= cid_end  {
                            let unicode = utf16be_to_string(&unicode_data);
                            map.insert(cid, unicode);
                            *unicode_data.last_mut().unwrap() += 1;
                        }
                    }
                    (Ok(Primitive::String(cid_start_data)), Ok(Primitive::String(cid_end_data)), Ok(Primitive::Array(unicode_data_arr))) => {
                        let cid_start = u16::from_be_bytes(cid_start_data.as_bytes().try_into().unwrap());
                        let cid_end = u16::from_be_bytes(cid_end_data.as_bytes().try_into().unwrap());

                        for (cid, unicode_data) in (cid_start ..= cid_end).zip(unicode_data_arr) {
                            let unicode = utf16be_to_string(&unicode_data.as_string().unwrap().as_bytes());
                            map.insert(cid, unicode);
                        }
                    }
                    _ => break
                }
            }
            b"endcmap" => break,
            _ => {}
        }
    }

    map
}

struct FontInfo {
    font: RcRef<Font>,
    cmap: HashMap<u16, String>
}
struct Cache {
    fonts: HashMap<String, FontInfo>
}
impl Cache {
    fn new() -> Self {
        Cache {
            fonts: HashMap::new()
        }
    }
    fn add_font(&mut self, name: impl Into<String>, font: RcRef<Font>) {
        println!("add_font({:?})", font);
        if let Some(to_unicode) = font.to_unicode() {
            let cmap = parse_cmap(to_unicode.data().unwrap());
            self.fonts.insert(name.into(), FontInfo { font, cmap });
        }
    }
    fn get_font(&self, name: &str) -> Option<&FontInfo> {
        self.fonts.get(name)
    }
}

fn add_string(data: &[u8], out: &mut String, info: &FontInfo) {
    if let Some(encoding) = info.font.encoding() {
        match encoding.base {
            BaseEncoding::IdentityH => {
                for w in data.windows(2) {
                    let cp = u16::from_be_bytes(w.try_into().unwrap());
                    if let Some(s) = info.cmap.get(&cp) {
                        out.push_str(s);
                    }
                }
            }
            _ => {
                for &b in data {
                    if let Some(s) = info.cmap.get(&(b as u16)) {
                        out.push_str(s);
                    } else {
                        out.push(b as char);
                    }
                }
            }
        };
    }
}

fn main() -> Result<(), PdfError> {
    let path = args().nth(1).expect("no file given");
    println!("read: {}", path);
    let file = File::<Vec<u8>>::open(&path).unwrap();
    
    let mut out = String::new();
    for page in file.pages() {
        let page = page?;
        let resources = page.resources.as_ref().unwrap();
        let mut cache = Cache::new();
        
        // make sure all fonts are in the cache, so we can reference them
        for (name, &font) in &resources.fonts {
            cache.add_font(name, file.get(font)?);
        }
        for gs in resources.graphics_states.values() {
            if let Some((font, _)) = gs.font {
                let font = file.get(font)?;
                cache.add_font(font.name.clone(), font);
            }
        }
        let mut current_font = None;
        let contents = page.contents.as_ref().unwrap();
        for op in &contents.operations {
            match op {
                Op::GraphicsState { name } => {
                    let gs = resources.graphics_states.get(name).unwrap();
                    
                    if let Some((font, _)) = gs.font {
                        let font = file.get(font)?;
                        current_font = cache.get_font(&font.name);
                    }
                }
                // text font
                Op::TextFont { name, .. } => {
                    current_font = cache.get_font(name);
                }
                Op::TextDraw { text } => if let Some(font) = current_font {
                    add_string(&text.data, &mut out, font);
                }
                Op::TextDrawAdjusted { array } =>  if let Some(font) = current_font {
                    for data in array {
                        if let TextDrawAdjusted::Text(text) = data {
                            add_string(&text.data, &mut out, font);
                        }
                    }
                }
                Op::TextNewline => {
                    out.push('\n');
                }
                _ => {}
            }
        }
    }
    println!("{}", out);

    Ok(())
}
