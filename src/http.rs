//! `http`: probar APIs desde la terminal (sintaxis parecida a HTTPie).
//!   http :8000/api/leads                      GET a localhost:8000
//!   http POST :8000/api/leads nombre=Ana edad:=30 activo:=true
//!   http :8000/api/leads page==2 Authorization:"Bearer $TOKEN"
//! Campos: `nombre=Ana` (texto), `edad:=30` (JSON tal cual), `page==2` (?page=2), `Cabecera:valor`.
//! La respuesta: estado, tiempo, tamaño y el cuerpo — listas de objetos en tabla (también las
//! paginadas tipo Django REST `{"count", "results": [...]}`), el resto como JSON con colores.
//! Si la salida va a un pipe o archivo se escribe el cuerpo tal cual. `http` solo: historial.

use crate::table::{self, truncate, Align};
use crate::{home, Shell, BLUE, GRAY, GREEN, RED, RESET, YELLOW};
use serde_json::{Map, Value};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Debug, Default, PartialEq)]
struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    body: Option<Value>,
}

/// ":8000/x" → "http://localhost:8000/x"; "localhost:3000" → "http://…"; "api.com" → "https://api.com".
fn normalize_url(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return raw.to_string();
    }
    if let Some(rest) = raw.strip_prefix(':') {
        return format!("http://localhost:{rest}");
    }
    let host = raw.split(['/', ':']).next().unwrap_or("");
    let local = host == "localhost" || host.starts_with("127.") || host.starts_with("192.168.") || host.starts_with("10.") || host == "0.0.0.0";
    format!("{}://{raw}", if local { "http" } else { "https" })
}

/// Un argumento después de la URL. Se usa el separador que aparece primero (a igual posición,
/// el más largo): `a:=1` es JSON, `a==1` query, `a=1` texto, `A:b` cabecera.
enum Item {
    Header(String, String),
    Query(String, String),
    Field(String, Value),
}

fn parse_item(arg: &str) -> Option<Item> {
    let seps = [":=", "==", "=", ":"];
    let (pos, sep) = seps.iter().filter_map(|s| arg.find(s).map(|p| (p, *s))).min_by_key(|(p, s)| (*p, usize::MAX - s.len()))?;
    let (key, value) = (arg[..pos].to_string(), arg[pos + sep.len()..].to_string());
    if key.is_empty() {
        return None;
    }
    Some(match sep {
        ":=" => Item::Field(key, serde_json::from_str(&value).unwrap_or(Value::String(value))),
        "==" => Item::Query(key, value),
        "=" => Item::Field(key, Value::String(value)),
        _ => Item::Header(key, value.trim_start().to_string()),
    })
}

fn parse_request(args: &[String]) -> Result<Request, String> {
    let mut it = args.iter().peekable();
    let explicit = it.peek().map(|a| a.to_uppercase()).filter(|m| METHODS.contains(&m.as_str()));
    if explicit.is_some() {
        it.next();
    }
    let url = it.next().ok_or("falta la URL")?;
    let mut req = Request { url: normalize_url(url), ..Default::default() };
    let mut fields = Map::new();
    for arg in it {
        match parse_item(arg).ok_or_else(|| format!("no entiendo \"{arg}\" (usa nombre=valor, n:=30, p==2 o Cabecera:valor)"))? {
            Item::Header(k, v) => req.headers.push((k, v)),
            Item::Query(k, v) => req.query.push((k, v)),
            Item::Field(k, v) => {
                fields.insert(k, v);
            }
        }
    }
    if !fields.is_empty() {
        req.body = Some(Value::Object(fields));
    }
    // Sin método: GET, o POST si se mandan datos (como HTTPie).
    req.method = explicit.unwrap_or_else(|| if req.body.is_some() { "POST".into() } else { "GET".into() });
    Ok(req)
}

struct Response {
    status: u16,
    reason: String,
    content_type: String,
    body: Vec<u8>,
    took: Duration,
}

fn send(req: &Request) -> Result<Response, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("fysh/", env!("CARGO_PKG_VERSION")))
        .build();
    let mut r = agent.request(&req.method, &req.url);
    for (k, v) in &req.query {
        r = r.query(k, v);
    }
    for (k, v) in &req.headers {
        r = r.set(k, v);
    }
    let started = Instant::now();
    let result = match &req.body {
        Some(body) => r.set("Content-Type", "application/json").send_string(&body.to_string()),
        None => r.call(),
    };
    let resp = match result {
        Ok(resp) => resp,
        // 4xx/5xx también traen respuesta: se muestra igual.
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(ureq::Error::Transport(t)) => {
            let msg = t.to_string();
            let hint = if msg.contains("Connection refused") || msg.contains("connect") {
                " (¿el servidor está encendido? míralo con: puertos)"
            } else {
                ""
            };
            return Err(format!("{msg}{hint}"));
        }
    };
    let (status, reason) = (resp.status(), resp.status_text().to_string());
    let content_type = resp.content_type().to_string();
    let mut body = Vec::new();
    resp.into_reader().take(50 * 1024 * 1024).read_to_end(&mut body).map_err(|e| e.to_string())?;
    Ok(Response { status, reason, content_type, body, took: started.elapsed() })
}

fn size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Si el JSON es una lista de objetos (o un objeto con una lista de objetos adentro, como la
/// paginación de Django REST), la lista y los demás campos.
fn as_rows(v: &Value) -> Option<(&Vec<Value>, Vec<(String, &Value)>)> {
    let is_rows = |a: &Vec<Value>| !a.is_empty() && a.iter().all(Value::is_object);
    match v {
        Value::Array(a) if is_rows(a) => Some((a, Vec::new())),
        Value::Object(o) => {
            let lists: Vec<(&String, &Vec<Value>)> = o.iter().filter_map(|(k, v)| v.as_array().filter(|a| is_rows(a)).map(|a| (k, a))).collect();
            if let [(key, list)] = lists.as_slice() {
                let others = o.iter().filter(|(k, _)| k != key).map(|(k, v)| (k.clone(), v)).collect();
                Some((list, others))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn cell(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "—".into(),
        other => other.to_string(),
    }
}

/// Qué columnas caben en `width`: primero las de valores simples (id, nombre, email…), en su
/// orden; las que tienen objetos o listas adentro solo si sobra espacio.
fn fit_columns<'a>(cols: &'a [String], rows: &[Value], width: usize) -> Vec<&'a String> {
    const CELL: usize = 28;
    let col_width = |c: &str| rows.iter().map(|r| cell(&r[c]).chars().count().min(CELL)).max().unwrap_or(0).max(c.chars().count()) + 3;
    let nested = |c: &str| rows.iter().any(|r| r[c].is_object() || r[c].is_array());
    let mut order: Vec<&String> = cols.iter().filter(|c| !nested(c)).collect();
    order.extend(cols.iter().filter(|c| nested(c)));
    let mut used = 1;
    let mut chosen: Vec<&String> = Vec::new();
    for c in order {
        let w = col_width(c);
        if used + w > width && !chosen.is_empty() {
            continue;
        }
        used += w;
        chosen.push(c);
    }
    // Mostrar en el orden original de la API.
    cols.iter().filter(|c| chosen.contains(c)).collect()
}

fn print_rows(rows: &[Value]) {
    const MAX_ROWS: usize = 50;
    let mut cols: Vec<String> = Vec::new();
    for r in rows {
        for k in r.as_object().into_iter().flat_map(|o| o.keys()) {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    let width = crossterm::terminal::size().map_or(120, |(w, _)| w as usize);
    let shown = fit_columns(&cols, rows, width);
    let body: Vec<Vec<String>> = rows
        .iter()
        .take(MAX_ROWS)
        .map(|r| shown.iter().map(|c| truncate(&cell(&r[c.as_str()]), 28)).collect())
        .collect();
    let headers: Vec<&str> = shown.iter().map(|s| s.as_str()).collect();
    table::print(&headers, &vec![Align::Left; headers.len()], &body);
    let mut notes = vec![format!("{} elementos", rows.len())];
    if rows.len() > MAX_ROWS {
        notes.push(format!("se muestran {MAX_ROWS}"));
    }
    let hidden: Vec<&str> = cols.iter().filter(|c| !shown.contains(c)).map(String::as_str).collect();
    if !hidden.is_empty() {
        notes.push(format!("no caben: {}", hidden.join(", ")));
    }
    println!("{GRAY}{} · http … > respuesta.json para guardarla completa{RESET}", notes.join(" · "));
}

/// JSON con sangría y colores: claves azules, textos verdes, números amarillos, true/false/null rojos.
fn pretty(v: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match v {
        Value::Object(o) if !o.is_empty() => {
            out.push_str("{\n");
            for (i, (k, val)) in o.iter().enumerate() {
                out.push_str(&format!("{pad}  {BLUE}\"{k}\"{RESET}: "));
                pretty(val, indent + 1, out);
                out.push_str(if i + 1 < o.len() { ",\n" } else { "\n" });
            }
            out.push_str(&format!("{pad}}}"));
        }
        Value::Array(a) if !a.is_empty() => {
            out.push_str("[\n");
            for (i, val) in a.iter().enumerate() {
                out.push_str(&format!("{pad}  "));
                pretty(val, indent + 1, out);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&format!("{pad}]"));
        }
        Value::String(_) => out.push_str(&format!("{GREEN}{v}{RESET}")),
        Value::Number(_) => out.push_str(&format!("{YELLOW}{v}{RESET}")),
        Value::Bool(_) | Value::Null => out.push_str(&format!("{RED}{v}{RESET}")),
        other => out.push_str(&other.to_string()), // {} o [] vacíos
    }
}

fn show(resp: &Response) {
    let color = match resp.status {
        200..=299 => GREEN,
        300..=399 => YELLOW,
        _ => RED,
    };
    let ctype = resp.content_type.split(';').next().unwrap_or("");
    println!(
        "{color}{} {}{RESET} {GRAY}· {} ms · {} · {ctype}{RESET}",
        resp.status,
        resp.reason,
        resp.took.as_millis(),
        size(resp.body.len())
    );
    if resp.body.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(&resp.body);
    match serde_json::from_str::<Value>(&text) {
        Ok(json) => {
            if let Some((rows, others)) = as_rows(&json) {
                if !others.is_empty() {
                    let summary: Vec<String> = others.iter().map(|(k, v)| format!("{BLUE}{k}{RESET}={}", truncate(&cell(v), 60))).collect();
                    println!("{}", summary.join("  "));
                }
                print_rows(rows);
            } else {
                let mut out = String::new();
                pretty(&json, 0, &mut out);
                println!("{out}");
            }
        }
        Err(_) => println!("{text}"),
    }
}

// ---------- historial ----------

fn history_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("http-historial"))
}

fn remember(line: &str) {
    let Some(file) = history_file() else { return };
    let mut lines: Vec<String> = std::fs::read_to_string(&file).unwrap_or_default().lines().map(str::to_owned).collect();
    lines.retain(|l| l != line);
    lines.push(line.to_string());
    let keep = lines.len().saturating_sub(200);
    let _ = file.parent().map(std::fs::create_dir_all);
    let _ = std::fs::write(file, lines[keep..].join("\n") + "\n");
}

/// `http` solo: las peticiones anteriores (la más reciente arriba); la elegida queda en la línea.
fn history_picker(shell: &mut Shell) -> i32 {
    let lines: Vec<String> = history_file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default().lines().rev().map(str::to_owned).collect();
    if lines.is_empty() {
        println!("{GRAY}Todavía no hiciste peticiones. Ejemplo: http :8000/api/leads{RESET}");
        return 0;
    }
    let rows: Vec<Vec<String>> = lines.iter().map(|l| vec![truncate(l, 90)]).collect();
    if !std::io::stdin().is_terminal() {
        table::print(&["Petición"], &[Align::Left], &rows);
        return 0;
    }
    if let Some(i) = crate::picker::select_one(&["Petición (queda escrita en la línea para editarla o darle Enter)"], &rows) {
        shell.pending = Some(lines[i].clone());
    }
    0
}

/// `http [MÉTODO] URL [campos…]`
pub fn http_cmd(shell: &mut Shell, args: &[String]) -> i32 {
    if args.is_empty() {
        return history_picker(shell);
    }
    let req = match parse_request(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("http: {e}");
            eprintln!("{GRAY}ej: http :8000/api/leads · http POST :8000/api/leads nombre=Ana edad:=30{RESET}");
            return 2;
        }
    };
    // Esta petición sola (la línea puede traer otras con ; o &&), con comillas donde hagan falta.
    let quoted: Vec<String> = args.iter().map(|a| if a.contains([' ', '"', '\'']) { format!("\"{}\"", a.replace('"', "'")) } else { a.clone() }).collect();
    remember(&format!("http {}", quoted.join(" ")));
    let resp = match send(&req) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{RED}http: {} {}:{RESET} {e}", req.method, req.url);
            return 1;
        }
    };
    if std::io::stdout().is_terminal() {
        show(&resp);
    } else {
        // Hacia un pipe o archivo: el cuerpo tal cual (para `> datos.json` o `| findstr`).
        let _ = std::io::Write::write_all(&mut std::io::stdout(), &resp.body);
    }
    // Como curl --fail: 4xx/5xx cuentan como error para && y ||.
    if resp.status >= 400 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn completa_las_urls() {
        assert_eq!(normalize_url(":8000/api/leads"), "http://localhost:8000/api/leads");
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("127.0.0.1:8000/x"), "http://127.0.0.1:8000/x");
        assert_eq!(normalize_url("api.github.com/repos"), "https://api.github.com/repos");
        assert_eq!(normalize_url("https://x.com"), "https://x.com");
    }

    #[test]
    fn entiende_campos_cabeceras_y_query() {
        let r = parse_request(&args(&[":8000/api/leads", "nombre=Ana", "edad:=30", "tags:=[\"a\"]", "page==2", "Authorization:Bearer abc", "url=http://x.com"])).unwrap();
        assert_eq!(r.method, "POST", "con datos y sin método: POST");
        assert_eq!(r.query, [("page".to_string(), "2".to_string())]);
        assert_eq!(r.headers, [("Authorization".to_string(), "Bearer abc".to_string())]);
        let body = r.body.unwrap();
        assert_eq!(body["edad"], 30);
        assert_eq!(body["tags"][0], "a");
        assert_eq!(body["url"], "http://x.com", "el = va antes que el : de la URL");

        let g = parse_request(&args(&["delete", "localhost:8000/api/leads/5"])).unwrap();
        assert_eq!((g.method.as_str(), g.body.is_none()), ("DELETE", true));
        assert!(parse_request(&args(&["GET"])).is_err());
        assert!(parse_request(&args(&[":8000", "sinseparador"])).is_err());
    }

    #[test]
    fn detecta_listas_para_mostrar_en_tabla() {
        let plain: Value = serde_json::from_str(r#"[{"id":1,"nombre":"Ana"},{"id":2,"nombre":"Luis"}]"#).unwrap();
        assert_eq!(as_rows(&plain).unwrap().0.len(), 2);
        let paged: Value = serde_json::from_str(r#"{"count":2,"next":null,"results":[{"id":1},{"id":2}]}"#).unwrap();
        let (rows, others) = as_rows(&paged).unwrap();
        assert_eq!((rows.len(), others.len()), (2, 2));
        // Con poco ancho: primero las columnas simples; la de objetos solo si cabe.
        let users: Value = serde_json::from_str(r#"[{"address":{"city":"X"},"id":1,"name":"Leanne Graham","email":"a@b.c"}]"#).unwrap();
        let rows = users.as_array().unwrap();
        let cols: Vec<String> = ["address", "id", "name", "email"].iter().map(|s| s.to_string()).collect();
        assert_eq!(fit_columns(&cols, rows, 40), ["id", "name", "email"]);
        assert_eq!(fit_columns(&cols, rows, 200).len(), 4);

        let obj: Value = serde_json::from_str(r#"{"id":1,"tags":["a","b"]}"#).unwrap();
        assert!(as_rows(&obj).is_none());
        let mut out = String::new();
        pretty(&obj, 0, &mut out);
        assert_eq!(crate::table::strip_ansi(&out), "{\n  \"id\": 1,\n  \"tags\": [\n    \"a\",\n    \"b\"\n  ]\n}");
    }
}
