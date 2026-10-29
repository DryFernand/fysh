//! `compartir <archivo|carpeta>`: un servidor web temporal en tu red con un código QR en la terminal,
//! para bajar el archivo desde el celular (u otra PC). `compartir -recibir`: al revés, subir desde el
//! celular a la carpeta actual. Ctrl+C lo apaga. La dirección lleva una clave al azar: sin ella, nada.

use crate::archive::unique;
use crate::builtins::human_size;
use crate::script::INTERRUPTED;
use crate::{BOLD, GRAY, GREEN, RESET, YELLOW};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

struct Shared {
    token: String,
    items: Vec<PathBuf>,
    /// Carpeta donde se guarda lo que se sube; None = no se aceptan subidas.
    receive: Option<PathBuf>,
}

/// IP de esta PC en la red local. Conectar UDP no manda nada: solo elige la interfaz de salida.
fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// Clave al azar para la dirección (RandomState se siembra con aleatoriedad del sistema).
fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let a = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let b = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")[..20].to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
         <title>{t}</title><style>body{{font-family:system-ui,sans-serif;max-width:640px;margin:24px auto;padding:0 16px;\
         background:#111;color:#eee}}h1{{font-size:20px}}a{{color:#6cf;text-decoration:none}}li{{padding:10px 0;\
         border-bottom:1px solid #333;list-style:none;display:flex;justify-content:space-between;gap:12px}}ul{{padding:0}}\
         small{{color:#999}}button,input{{font-size:16px;padding:10px;margin:8px 0}}</style></head><body><h1>{t}</h1>{body}</body></html>",
        t = html_escape(title)
    )
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> io::Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(body)
}

fn not_found(stream: &mut TcpStream) -> io::Result<()> {
    respond(stream, "404 Not Found", "text/plain; charset=utf-8", b"No encontrado")
}

/// Ruta pedida dentro de un elemento compartido, sin salirse de él (nada de ..).
fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let full = root.join(rel);
    let canon = full.canonicalize().ok()?;
    canon.starts_with(root.canonicalize().ok()?).then_some(full)
}

fn list_html(shared: &Shared) -> String {
    let mut body = String::from("<ul>");
    for (i, item) in shared.items.iter().enumerate() {
        let name = item.file_name().unwrap_or_default().to_string_lossy().to_string();
        let size = if item.is_dir() { "carpeta".to_string() } else { human_size(item.metadata().map_or(0, |m| m.len())) };
        let slash = if item.is_dir() { "/" } else { "" };
        body += &format!("<li><a href='{i}{slash}'>{}{slash}</a><small>{size}</small></li>", html_escape(&name));
    }
    body + "</ul>"
}

fn dir_html(dir: &Path, title: &str) -> String {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map(|r| r.flatten().collect()).unwrap_or_default();
    entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name().to_string_lossy().to_lowercase()));
    let mut body = String::from("<p><a href='../'>⬑ volver</a></p><ul>");
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = e.path().is_dir();
        let size = if is_dir { "carpeta".to_string() } else { human_size(e.metadata().map_or(0, |m| m.len())) };
        let slash = if is_dir { "/" } else { "" };
        body += &format!("<li><a href='{}{slash}'>{}{slash}</a><small>{size}</small></li>", percent_encode(&name), html_escape(&name));
    }
    page(title, &(body + "</ul>"))
}

const UPLOAD_HTML: &str = "<input type=file id=f multiple><br><button onclick=send()>Enviar</button><p id=s></p>\
<script>async function send(){const fs=document.getElementById('f').files,s=document.getElementById('s');let ok=0;\
for(const f of fs){s.textContent='Enviando '+f.name+'…';const r=await fetch('subir?nombre='+encodeURIComponent(f.name),\
{method:'POST',body:f});if(r.ok)ok++;else s.textContent='✗ '+f.name}s.textContent='✓ Enviados '+ok+' de '+fs.length}</script>";

/// Nombre de archivo subido, sin rutas ni caracteres raros.
fn clean_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next()?.trim();
    let name: String = name.chars().filter(|c| !c.is_control() && !r#":*?"<>|"#.contains(*c)).collect();
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

fn handle(mut stream: TcpStream, shared: &Shared) -> io::Result<()> {
    let peer = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut length = 0u64;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let Some(rest) = path.strip_prefix(&format!("/{}/", shared.token)) else {
        return not_found(&mut stream);
    };
    let rest = percent_decode(rest);

    // Subir un archivo (compartir -recibir).
    if method == "POST" && rest == "subir" {
        let Some(dir) = &shared.receive else { return not_found(&mut stream) };
        let name = query.split('&').find_map(|kv| kv.strip_prefix("nombre=")).map(percent_decode).and_then(|n| clean_name(&n));
        let Some(name) = name else { return respond(&mut stream, "400 Bad Request", "text/plain", b"falta el nombre") };
        let dest = unique(dir.join(&name));
        let copied = io::copy(&mut (&mut reader).take(length), &mut std::fs::File::create(&dest)?)?;
        if copied < length {
            let _ = std::fs::remove_file(&dest); // se cortó a la mitad
            return Ok(());
        }
        println!("{GREEN}↑{RESET} {} {GRAY}({}, desde {peer}){RESET}", dest.file_name().unwrap_or_default().to_string_lossy(), human_size(copied));
        return respond(&mut stream, "200 OK", "text/plain", b"ok");
    }

    if rest.is_empty() {
        let mut body = list_html(shared);
        if shared.receive.is_some() {
            body = format!("{UPLOAD_HTML}{}", if shared.items.is_empty() { String::new() } else { body });
        }
        let title = if shared.receive.is_some() { "Enviar a fysh" } else { "Compartido desde fysh" };
        return respond(&mut stream, "200 OK", "text/html; charset=utf-8", page(title, &body).as_bytes());
    }

    // "<n>/<ruta dentro>": un elemento compartido o algo dentro de una carpeta compartida.
    let (index, inner) = rest.split_once('/').unwrap_or((&rest, ""));
    let Some(root) = index.parse::<usize>().ok().and_then(|i| shared.items.get(i)) else { return not_found(&mut stream) };
    let full = if inner.is_empty() { Some(root.clone()) } else { resolve(root, inner.trim_end_matches('/')) };
    let Some(full) = full else { return not_found(&mut stream) };
    if full.is_dir() {
        if !path.ends_with('/') {
            write!(stream, "HTTP/1.1 301 Moved\r\nLocation: {path}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            return Ok(());
        }
        let title = full.file_name().unwrap_or_default().to_string_lossy().to_string();
        return respond(&mut stream, "200 OK", "text/html; charset=utf-8", dir_html(&full, &title).as_bytes());
    }
    let mut file = std::fs::File::open(&full)?;
    let size = file.metadata()?.len();
    let name = full.file_name().unwrap_or_default().to_string_lossy().to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {size}\r\n\
         Content-Disposition: attachment; filename*=UTF-8''{}\r\nConnection: close\r\n\r\n",
        percent_encode(&name)
    )?;
    io::copy(&mut file, &mut stream)?;
    println!("{GREEN}↓{RESET} {name} {GRAY}({}, a {peer}){RESET}", human_size(size));
    Ok(())
}

/// El QR con medio bloque por módulo: dos filas del código por línea de terminal.
fn qr(text: &str) -> Option<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(text).ok()?;
    Some(code.render::<Dense1x2>().dark_color(Dense1x2::Light).light_color(Dense1x2::Dark).build())
}

pub fn compartir_cmd(args: &[String]) -> i32 {
    let receive = args.iter().any(|a| a == "-recibir");
    let paths: Vec<&String> = args.iter().filter(|a| *a != "-recibir").collect();
    if paths.is_empty() && !receive {
        eprintln!("{GRAY}uso: compartir <archivo o carpeta>...   lo baja el celular escaneando el QR{RESET}");
        eprintln!("{GRAY}     compartir -recibir                  el celular sube archivos a esta carpeta{RESET}");
        return 1;
    }
    let mut items = Vec::new();
    for p in paths {
        match std::path::absolute(p) {
            Ok(a) if a.exists() => items.push(a),
            _ => {
                eprintln!("compartir: {p}: no existe");
                return 1;
            }
        }
    }
    let listener = match TcpListener::bind(("0.0.0.0", 0)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("compartir: {}", crate::builtins::error_msg(&e));
            return 1;
        }
    };
    let port = listener.local_addr().map_or(0, |a| a.port());
    let Some(ip) = local_ip() else {
        eprintln!("compartir: no estás conectado a ninguna red");
        return 1;
    };
    let shared = Arc::new(Shared { token: token(), items, receive: receive.then(|| std::env::current_dir().unwrap_or_default()) });
    let url = format!("http://{ip}:{port}/{}/", shared.token);
    if let Some(code) = qr(&url) {
        println!("{code}");
    }
    let what = match shared.items.len() {
        0 => "Para enviar archivos a esta carpeta".to_string(),
        1 => format!("Para bajar {}", shared.items[0].file_name().unwrap_or_default().to_string_lossy()),
        n => format!("Para bajar {n} elementos"),
    };
    let what = if receive && !shared.items.is_empty() { format!("{what} o enviar archivos aquí") } else { what };
    println!("{BOLD}{what}{RESET}: escanea el QR o abre {YELLOW}{url}{RESET}");
    println!("{GRAY}Solo en tu misma red (Wi-Fi). Ctrl+C para terminar.{RESET}");
    if cfg!(windows) {
        println!("{GRAY}Si Windows pregunta por el firewall, permite el acceso en redes privadas.{RESET}");
    }
    listener.set_nonblocking(true).ok();
    INTERRUPTED.store(false, Ordering::Relaxed);
    loop {
        if INTERRUPTED.swap(false, Ordering::Relaxed) {
            println!("{GRAY}Dejé de compartir{RESET}");
            return 0;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).ok();
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    let _ = handle(stream, &shared);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas_y_nombres_seguros() {
        assert_eq!(percent_decode("mi%20foto%C3%B1.jpg"), "mi fotoñ.jpg");
        assert_eq!(percent_encode("a b/ñ"), "a%20b%2F%C3%B1");
        assert_eq!(clean_name("C:\\x\\..\\foto.jpg"), Some("foto.jpg".into()));
        assert_eq!(clean_name("../.."), None);
        let dir = std::env::temp_dir();
        assert!(resolve(&dir, "../secreto").is_none(), "no se puede salir de lo compartido");
        assert_eq!(token().len(), 20);
        assert_ne!(token(), token());
    }

    #[test]
    fn sirve_y_recibe_archivos() {
        let base = std::env::temp_dir().join(format!("fysh-compartir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("recibidos")).unwrap();
        std::fs::write(base.join("hola.txt"), "contenido").unwrap();
        let shared = Arc::new(Shared { token: "clave".into(), items: vec![base.join("hola.txt")], receive: Some(base.join("recibidos")) });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = Arc::clone(&shared);
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                let _ = handle(s, &server);
            }
        });
        let get = |req: &str| {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        assert!(get("GET /clave/0 HTTP/1.1\r\n\r\n").ends_with("contenido"));
        assert!(get("GET /otra/0 HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"), "sin la clave no hay nada");
        assert!(get("GET /clave/ HTTP/1.1\r\n\r\n").contains("hola.txt"));
        get("POST /clave/subir?nombre=..%2Fnuevo.txt HTTP/1.1\r\nContent-Length: 4\r\n\r\ndato");
        assert_eq!(std::fs::read_to_string(base.join("recibidos").join("nuevo.txt")).unwrap(), "dato");
        std::fs::remove_dir_all(&base).unwrap();
    }
}
