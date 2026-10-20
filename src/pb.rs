//! Protocol buffers without a schema, just enough for Antigravity: reading fields by number, and
//! writing messages.

/// A field's value: a number, or bytes (text, or a message inside).
#[derive(Clone, Copy, Debug)]
pub enum Val<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut n = 0u64;
    for shift in (0..64).step_by(7) {
        let c = *b.get(*i)?;
        *i += 1;
        n |= u64::from(c & 0x7f) << shift;
        if c < 0x80 {
            return Some(n);
        }
    }
    None
}

/// The fields of a message, in order, or None if `b` isn't one.
pub fn fields(b: &[u8]) -> Option<Vec<(u32, Val<'_>)>> {
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let num = u32::try_from(key >> 3).ok()?;
        let val = match key & 7 {
            0 => Val::Int(varint(b, &mut i)?),
            1 => {
                i += 8;
                Val::Bytes(b.get(i - 8..i)?)
            }
            2 => {
                let n = usize::try_from(varint(b, &mut i)?).ok()?;
                i += n;
                Val::Bytes(b.get(i - n..i)?)
            }
            5 => {
                i += 4;
                Val::Bytes(b.get(i - 4..i)?)
            }
            _ => return None,
        };
        out.push((num, val));
    }
    Some(out)
}

/// Every message (or text) found at `path` (field numbers, outermost first).
pub fn all<'a>(b: &'a [u8], path: &[u32]) -> Vec<&'a [u8]> {
    let Some((first, rest)) = path.split_first() else {
        return vec![b];
    };
    fields(b)
        .unwrap_or_default()
        .into_iter()
        .filter(|(n, _)| n == first)
        .filter_map(|(_, v)| match v {
            Val::Bytes(inner) => Some(inner),
            Val::Int(_) => None,
        })
        .flat_map(|inner| all(inner, rest))
        .collect()
}

pub fn get<'a>(b: &'a [u8], path: &[u32]) -> Option<&'a [u8]> {
    all(b, path).into_iter().next()
}

pub fn text(b: &[u8], path: &[u32]) -> Option<String> {
    get(b, path).map(|t| String::from_utf8_lossy(t).into_owned())
}

pub fn int(b: &[u8], path: &[u32]) -> Option<u64> {
    let (last, outer) = path.split_last()?;
    let msg = get(b, outer)?;
    fields(msg)?.into_iter().find_map(|(n, v)| match v {
        Val::Int(x) if n == *last => Some(x),
        _ => None,
    })
}

/// A message being written: `Msg::new().int(1, 14).text(2, "hi").msg(5, other)`.
#[derive(Clone, Default)]
pub struct Msg(Vec<u8>);

impl Msg {
    pub fn new() -> Msg {
        Msg::default()
    }

    fn key(&mut self, num: u32, wire: u64) {
        self.put(u64::from(num) << 3 | wire);
    }

    fn put(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.0.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        self.0.push(n as u8);
    }

    pub fn int(mut self, num: u32, v: u64) -> Msg {
        self.key(num, 0);
        self.put(v);
        self
    }

    pub fn bytes(mut self, num: u32, v: &[u8]) -> Msg {
        self.key(num, 2);
        self.put(v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }

    pub fn text(self, num: u32, v: &str) -> Msg {
        self.bytes(num, v.as_bytes())
    }

    pub fn msg(self, num: u32, v: &Msg) -> Msg {
        self.bytes(num, &v.0)
    }

    pub fn done(self) -> Vec<u8> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let inner = Msg::new().text(1, "call_1").text(2, "run_command");
        let m = Msg::new()
            .int(1, 300)
            .msg(20, &inner)
            .msg(20, &Msg::new().text(1, "call_2"))
            .done();
        assert_eq!(int(&m, &[1]), Some(300));
        assert_eq!(text(&m, &[20, 2]).as_deref(), Some("run_command"));
        assert_eq!(all(&m, &[20, 1]).len(), 2);
        assert!(fields(b"\xff\xff").is_none());
    }
}
