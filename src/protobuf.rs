//! Lo mínimo del formato protobuf para editar el índice de conversaciones de agy
//! (jetbox_summaries_proto.pb) sin conocer su esquema completo:
//!   archivo = lista de entradas (campo 1), una por conversación
//!   entrada = { 1: id de la conversación, 2: resumen { 1: título, … } }
//! Solo se reescriben los bytes de la entrada que cambia; el resto se copia tal cual.

/// Un campo de un mensaje: número, tipo y dónde están sus bytes.
#[derive(Debug, Clone, Copy)]
struct Field {
    num: u64,
    wire: u8,
    start: usize,
    data: usize,
    end: usize,
}

fn read_varint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(i)?;
        i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, i));
        }
    }
    None
}

fn write_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Todos los campos del mensaje `b`. None si no es un mensaje válido.
fn parse(b: &[u8]) -> Option<Vec<Field>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let (key, j) = read_varint(b, i)?;
        let (num, wire) = (key >> 3, (key & 7) as u8);
        let (data, end) = match wire {
            0 => (j, read_varint(b, j)?.1),
            1 => (j, j + 8),
            2 => {
                let (len, k) = read_varint(b, j)?;
                (k, k.checked_add(len as usize)?)
            }
            5 => (j, j + 4),
            _ => return None,
        };
        if end > b.len() {
            return None;
        }
        out.push(Field { num, wire, start: i, data, end });
        i = end;
    }
    Some(out)
}

/// Campo de bytes/texto/mensaje: clave + largo + datos.
fn encode(num: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 6);
    write_varint((num << 3) | 2, &mut out);
    write_varint(data.len() as u64, &mut out);
    out.extend_from_slice(data);
    out
}

/// Id de conversación de una entrada (su campo 1).
fn entry_id<'a>(b: &'a [u8], entry: &Field) -> Option<&'a [u8]> {
    let inner = &b[entry.data..entry.end];
    let fields = parse(inner)?;
    let id = fields.iter().find(|f| f.num == 1 && f.wire == 2)?;
    Some(&inner[id.data..id.end])
}

/// Quita las entradas de esas conversaciones. Devuelve el archivo nuevo y cuántas quitó.
pub fn remove_entries(file: &[u8], ids: &[&str]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::with_capacity(file.len());
    let mut removed = 0;
    for f in parse(file)? {
        let is_target = f.num == 1 && f.wire == 2 && entry_id(file, &f).is_some_and(|id| ids.iter().any(|t| t.as_bytes() == id));
        if is_target {
            removed += 1;
        } else {
            out.extend_from_slice(&file[f.start..f.end]);
        }
    }
    Some((out, removed))
}

/// Cambia el título (resumen → campo 1) de una conversación. None si no la encuentra
/// o si el archivo no tiene la forma esperada (entonces no se toca nada).
pub fn rename_entry(file: &[u8], id: &str, title: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(file.len() + title.len());
    let mut done = false;
    for f in parse(file)? {
        if done || f.num != 1 || f.wire != 2 || entry_id(file, &f) != Some(id.as_bytes()) {
            out.extend_from_slice(&file[f.start..f.end]);
            continue;
        }
        let entry = &file[f.data..f.end];
        let mut new_entry = Vec::with_capacity(entry.len() + title.len());
        for ef in parse(entry)? {
            if ef.num != 2 || ef.wire != 2 {
                new_entry.extend_from_slice(&entry[ef.start..ef.end]);
                continue;
            }
            let summary = &entry[ef.data..ef.end];
            let mut new_summary = encode(1, title.as_bytes());
            for sf in parse(summary)? {
                if !(sf.num == 1 && sf.wire == 2) {
                    new_summary.extend_from_slice(&summary[sf.start..sf.end]);
                }
            }
            new_entry.extend(encode(2, &new_summary));
            done = true;
        }
        out.extend(encode(1, &new_entry));
    }
    done.then_some(out)
}

/// Título actual de una conversación (para las pruebas y para verificar después de cambiarlo).
pub fn title_of(file: &[u8], id: &str) -> Option<String> {
    for f in parse(file)? {
        if f.num == 1 && f.wire == 2 && entry_id(file, &f) == Some(id.as_bytes()) {
            let entry = &file[f.data..f.end];
            let summary = parse(entry)?.into_iter().find(|e| e.num == 2 && e.wire == 2)?;
            let s = &entry[summary.data..summary.end];
            let t = parse(s)?.into_iter().find(|x| x.num == 1 && x.wire == 2)?;
            return Some(String::from_utf8_lossy(&s[t.data..t.end]).into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entrada como las de agy: id, y resumen con título, un número y otro texto.
    fn entry(id: &str, title: &str) -> Vec<u8> {
        let mut summary = encode(1, title.as_bytes());
        summary.extend([2 << 3, 1]); // campo 2, varint 1
        summary.extend(encode(4, b"otro-dato"));
        let mut e = encode(1, id.as_bytes());
        e.extend(encode(2, &summary));
        encode(1, &e)
    }

    #[test]
    fn renombra_solo_la_entrada_pedida() {
        let a = entry("aaa", "Título viejo");
        let b = entry("bbb", "Otra");
        let file = [a.clone(), b.clone()].concat();
        let largo = "Un título bastante más largo que el anterior, con tildes: ñandú";
        let new = rename_entry(&file, "aaa", largo).unwrap();
        assert_eq!(title_of(&new, "aaa").as_deref(), Some(largo));
        assert_eq!(title_of(&new, "bbb").as_deref(), Some("Otra"));
        assert!(new.ends_with(&b), "las demás entradas quedan byte por byte igual");
        assert!(new.windows(9).any(|w| w == b"otro-dato"), "el resto del resumen se conserva");
        assert!(rename_entry(&file, "zzz", "x").is_none(), "si no existe, no toca nada");
    }

    #[test]
    fn borra_entradas_y_deja_el_resto_igual() {
        let file = [entry("aaa", "A"), entry("bbb", "B"), entry("ccc", "C")].concat();
        let (new, n) = remove_entries(&file, &["aaa", "ccc"]).unwrap();
        assert_eq!(n, 2);
        assert_eq!(new, entry("bbb", "B"));
        assert!(remove_entries(b"\xff\xff", &["x"]).is_none(), "archivo inválido: no se toca");
    }

    #[test]
    fn varints_grandes() {
        let mut v = Vec::new();
        write_varint(300, &mut v);
        assert_eq!(v, [0xac, 0x02]);
        assert_eq!(read_varint(&v, 0), Some((300, 2)));
    }
}
