use crate as pdf;
use crate::object::*;
use crate::error::*;

#[derive(Object, Debug)]
pub struct IccInfo {
    #[pdf(key="N")]
    pub components: u32,

    #[pdf(key="Alternate")]
    pub alternate: Option<Rc<ColorSpace>>,

    #[pdf(key="Range")]
    pub range: Option<Vec<f32>>,

    #[pdf(key="Metadata")]
    pub metadata: Option<Stream<()>>,
}

#[derive(Debug)]
pub enum ColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    Indexed(Rc<ColorSpace>, Vec<u8>),
    Separation(String, Rc<ColorSpace>, Function),
    Icc(Box<Stream<IccInfo>>),
    Other(Vec<Primitive>)
}


fn get_index(arr: &[Primitive], idx: usize) -> Result<&Primitive> {
     arr.get(idx).ok_or(PdfError::Bounds { index: idx, len: arr.len() })
}

impl Object for ColorSpace {
    fn serialize<W: io::Write>(&self, _out: &mut W) -> Result<()> {
        unimplemented!()
    }
    fn from_primitive(p: Primitive, resolve: &impl Resolve) -> Result<ColorSpace> {
        if let Ok(name) = p.as_name() {
            let cs = match name {
                "DeviceGray" => ColorSpace::DeviceGray,
                "DeviceRGB" => ColorSpace::DeviceRGB,
                "DeviceCMYK" => ColorSpace::DeviceCMYK,
                _ => unimplemented!()
            };
            return Ok(cs);
        }
        let arr = t!(p.into_array(resolve));
        let typ = t!(t!(get_index(&arr, 0)).as_name());
        
        match typ {
            "Indexed" => {
                let base = t!(Object::from_primitive(t!(get_index(&arr, 1)).clone(), resolve));
                let lookup = match t!(get_index(&arr, 3)) {
                    &Primitive::Reference(r) => resolve.resolve(r)?,
                    p => p.clone()
                };
                let lookup = match lookup {
                    Primitive::String(string) => string.into_bytes(),
                    Primitive::Stream(stream) => {
                        let s = Stream::<()>::from_stream(stream, resolve)?;
                        t!(s.decode()).into_owned()
                    },
                    p => return Err(PdfError::UnexpectedPrimitive {
                        expected: "String or Stream",
                        found: p.get_debug_name()
                    })
                };
                Ok(ColorSpace::Indexed(base, lookup))
            }
            "Separation" => {
                let name = t!(t!(get_index(&arr, 1)).clone().into_name());
                let alternate = t!(Object::from_primitive(t!(get_index(&arr, 2)).clone(), resolve));
                let tint = t!(Function::from_primitive(t!(get_index(&arr, 3)).clone(), resolve));
                Ok(ColorSpace::Separation(name, alternate, tint))
            }
            "ICCBased" => {
                let s: Stream<IccInfo> = t!(Stream::from_primitive(t!(get_index(&arr, 1)).clone(), resolve));
                Ok(ColorSpace::Icc(Box::new(s)))
            }
            _ => Ok(ColorSpace::Other(arr))
        }
    }
}
