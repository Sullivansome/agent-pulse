//! CoreText shapes the macOS system font, including fallback scripts and kerning.
use core_foundation::{
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use core_graphics::{
    base::kCGImageAlphaPremultipliedLast, color_space::CGColorSpace, context::CGContext,
};
use core_text::{
    font::{kCTFontEmphasizedSystemFontType, kCTFontSystemFontType, new_ui_font_for_language},
    line::CTLine,
    string_attributes::{kCTFontAttributeName, kCTForegroundColorFromContextAttributeName},
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<u8>,
}
type Key = (String, u32, u32, bool);
thread_local! { static CACHE: RefCell<HashMap<Key, Rc<Mask>>> = RefCell::default(); }

pub fn rasterize(text: &str, size: f32, width: f32, emphasized: bool) -> Rc<Mask> {
    let key = (text.into(), size.to_bits(), width.to_bits(), emphasized);
    CACHE.with(|cache| {
        if let Some(mask) = cache.borrow().get(&key) {
            return mask.clone();
        }
        let font = new_ui_font_for_language(
            if emphasized {
                kCTFontEmphasizedSystemFontType
            } else {
                kCTFontSystemFontType
            },
            size as f64,
            None,
        );
        let line_for = |text: &str| {
            let mut value = CFMutableAttributedString::new();
            value.replace_str(&CFString::new(text), CFRange::init(0, 0));
            let range = CFRange::init(0, value.char_len());
            // These are immutable CoreText attribute-name constants.
            unsafe {
                value.set_attribute(range, kCTFontAttributeName, &font);
                value.set_attribute(
                    range,
                    kCTForegroundColorFromContextAttributeName,
                    &CFBoolean::true_value(),
                );
            }
            CTLine::new_with_attributed_string(value.as_concrete_TypeRef())
        };
        let mut line = line_for(text);
        if line.get_typographic_bounds().width > width as f64 {
            let mut shortened = text.to_owned();
            while !shortened.is_empty() {
                shortened.pop();
                line = line_for(&format!("{shortened}…"));
                if line.get_typographic_bounds().width <= width as f64 {
                    break;
                }
            }
        }
        let w = width.ceil().max(1.0) as usize;
        let h = (size * 1.5).ceil().max(1.0) as usize;
        let mut context = CGContext::create_bitmap_context(
            None,
            w,
            h,
            8,
            w * 4,
            &CGColorSpace::create_device_rgb(),
            kCGImageAlphaPremultipliedLast,
        );
        context.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);
        context.set_text_position(0.0, h as f64 - size as f64);
        line.draw(&context);
        let mask = Rc::new(Mask {
            width: w,
            height: h,
            alpha: context.data().chunks_exact(4).map(|p| p[3]).collect(),
        });
        let mut cache = cache.borrow_mut();
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(key, mask.clone());
        mask
    })
}
