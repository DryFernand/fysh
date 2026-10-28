//! `db`: conexiones guardadas a MySQL/MariaDB y PostgreSQL. Al elegir una: abrir la consola
//! (mysql / psql), ver sus tablas y las primeras filas en una tabla de fysh, o probarla.
//! Si la conexión pasa por un túnel (`tunel`), se activa solo antes de conectar.
//! Se guardan en ~/.fysh/db.json **sin la contraseña**: esa va al almacén seguro del sistema
//! (Administrador de credenciales de Windows / llavero de Mac / llavero del usuario en Linux).

use crate::ask::{ask, confirm, password};
use crate::picker;
use crate::table::{self, truncate, Align};
use crate::{home, GRAY, GREEN, RED, RESET, YELLOW};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

const KEYRING_SERVICE: &str = "fysh-db";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    MySql,
    Postgres,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Kind::MySql => "mysql",
            Kind::Postgres => "postgres",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::MySql => "MySQL",
            Kind::Postgres => "PostgreSQL",
        }
    }
    fn default_port(self) -> u16 {
        match self {
            Kind::MySql => 3306,
            Kind::Postgres => 5432,
        }
    }
    fn default_user(self) -> &'static str {
        match self {
            Kind::MySql => "root",
            Kind::Postgres => "postgres",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Conn {
    name: String,
    kind: Kind,
    host: String,
    port: u16,
    user: String,
    database: String,
    /// Túnel por el que se llega (entonces se conecta a 127.0.0.1:<puerto del túnel>).
    tunnel: Option<String>,
}

impl Conn {
    fn target(&self) -> String {
        let db = if self.database.is_empty() { String::new() } else { format!("/{}", self.database) };
        match &self.tunnel {
            Some(t) => format!("{}@túnel {t}{db}", self.user),
            None => format!("{}@{}:{}{db}", self.user, self.host, self.port),
        }
    }

    fn to_json(&self) -> Value {
        json!({"name": self.name, "kind": self.kind.id(), "host": self.host, "port": self.port,
               "user": self.user, "database": self.database, "tunnel": self.tunnel})
    }

    fn from_json(v: &Value) -> Option<Self> {
        let kind = match v["kind"].as_str()? {
            "mysql" => Kind::MySql,
            "postgres" => Kind::Postgres,
            _ => return None,
        };
        Some(Conn {
            name: v["name"].as_str()?.into(),
            kind,
            host: v["host"].as_str().unwrap_or("localhost").into(),
            port: v["port"].as_u64().map_or(kind.default_port(), |p| p as u16),
            user: v["user"].as_str().unwrap_or("").into(),
            database: v["database"].as_str().unwrap_or("").into(),
            tunnel: v["tunnel"].as_str().map(str::to_owned),
        })
    }
}

fn data_file() -> Option<PathBuf> {
    home().map(|h| h.join(".fysh").join("db.json"))
}

fn load() -> Vec<Conn> {
    data_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
        .map(|list| list.iter().filter_map(Conn::from_json).collect())
        .unwrap_or_default()
}

fn save(list: &[Conn]) -> std::io::Result<()> {
    let file = data_file().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "sin carpeta de usuario"))?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(&list.iter().map(Conn::to_json).collect::<Vec<_>>()).unwrap_or_default())
}

// ---------- contraseñas en el almacén seguro ----------

fn get_password(name: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, name).ok()?.get_password().ok()
}

fn set_password(name: &str, pass: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, name).map_err(|e| e.to_string())?;
    if pass.is_empty() {
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry.set_password(pass).map_err(|e| e.to_string())
}

fn delete_password(name: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, name) {
        let _ = entry.delete_credential();
    }
}

// ---------- clientes mysql / psql ----------

/// El cliente de línea de comandos: en el PATH o, en Windows, donde lo deja el instalador.
fn client(kind: Kind) -> Option<PathBuf> {
    let (name, pattern) = match kind {
        Kind::MySql => ("mysql", "C:/Program Files/MySQL/MySQL Server */bin/mysql.exe"),
        Kind::Postgres => ("psql", "C:/Program Files/PostgreSQL/*/bin/psql.exe"),
    };
    crate::which(name).or_else(|| {
        if !cfg!(windows) {
            return None;
        }
        let mut found: Vec<PathBuf> = glob::glob(pattern).ok()?.filter_map(Result::ok).collect();
        found.sort();
        found.pop() // la versión más nueva
    })
}

/// Host y puerto a usar: los de la conexión, o 127.0.0.1 y el puerto del túnel (activándolo).
fn endpoint(c: &Conn) -> Result<(String, u16), String> {
    match &c.tunnel {
        Some(t) => crate::tunnel::ensure_active(t).map(|port| ("127.0.0.1".to_string(), port)),
        None => Ok((c.host.clone(), c.port)),
    }
}

/// Argumentos de conexión. `database` puede reemplazar la de la conexión (al elegir una de la lista).
fn connect_args(c: &Conn, host: &str, port: u16, database: &str) -> Vec<String> {
    let mut args: Vec<String> = match c.kind {
        Kind::MySql => vec!["-h".into(), host.into(), "-P".into(), port.to_string(), "--protocol=TCP".into(), "--connect-timeout=8".into()],
        Kind::Postgres => vec!["-h".into(), host.into(), "-p".into(), port.to_string()],
    };
    if !c.user.is_empty() {
        args.extend([if c.kind == Kind::MySql { "-u" } else { "-U" }.to_string(), c.user.clone()]);
    }
    match (c.kind, database) {
        (_, "") if c.kind == Kind::Postgres => args.extend(["-d".into(), "postgres".into()]),
        (_, "") => {}
        (Kind::MySql, db) => args.extend(["-D".into(), db.into()]),
        (Kind::Postgres, db) => args.extend(["-d".into(), db.into()]),
    }
    args
}

/// Prepara el comando del cliente con la contraseña por variable de entorno (no queda a la vista).
fn command(c: &Conn, database: &str) -> Result<Command, String> {
    let program = client(c.kind).ok_or_else(|| {
        format!("no encontré el cliente de {} ({}). Instálalo o agrégalo al PATH con: path", c.kind.label(), if c.kind == Kind::MySql { "mysql" } else { "psql" })
    })?;
    let (host, port) = endpoint(c)?;
    let mut cmd = Command::new(program);
    cmd.args(connect_args(c, &host, port, database));
    if let Some(pass) = get_password(&c.name) {
        cmd.env(if c.kind == Kind::MySql { "MYSQL_PWD" } else { "PGPASSWORD" }, pass);
    }
    if c.kind == Kind::Postgres {
        cmd.env("PGCONNECT_TIMEOUT", "8");
    }
    Ok(cmd)
}

/// Ejecuta una consulta y devuelve (columnas, filas).
fn query(c: &Conn, database: &str, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut cmd = command(c, database)?;
    match c.kind {
        Kind::MySql => cmd.args(["-B", "-e", sql]),
        Kind::Postgres => cmd.args(["-A", "-F", "\t", "-P", "footer=off", "-c", sql]),
    };
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().find(|l| !l.trim().is_empty() && !l.contains("[Warning]")).unwrap_or("error desconocido").trim().to_string());
    }
    Ok(parse_tsv(&String::from_utf8_lossy(&out.stdout)))
}

/// Salida separada por tabulaciones (primera línea = columnas).
fn parse_tsv(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = text.lines().filter(|l| !l.is_empty());
    let headers: Vec<String> = lines.next().map(|l| l.split('\t').map(str::to_owned).collect()).unwrap_or_default();
    let rows = lines.map(|l| l.split('\t').map(str::to_owned).collect()).collect();
    (headers, rows)
}

/// Muestra el resultado de una consulta en una tabla (como mucho 8 columnas a la vista).
fn print_result(headers: &[String], rows: &[Vec<String>]) {
    const MAX_COLS: usize = 8;
    let cols = headers.len().min(MAX_COLS);
    let head: Vec<&str> = headers[..cols].iter().map(String::as_str).collect();
    let body: Vec<Vec<String>> = rows.iter().map(|r| r.iter().take(cols).map(|v| truncate(v, 28)).collect()).collect();
    table::print(&head, &vec![Align::Left; cols], &body);
    if headers.len() > MAX_COLS {
        println!("{GRAY}… y {} columnas más (ábrela en la consola para verlas todas){RESET}", headers.len() - MAX_COLS);
    }
}

// ---------- comando ----------

/// `db` (selector) o `db <nombre>` (abre la consola de esa conexión).
pub fn db_cmd(args: &[String]) -> i32 {
    let mut list = load();
    if let Some(name) = args.first() {
        return match list.iter().find(|c| &c.name == name) {
            Some(c) => console(c),
            None => {
                eprintln!("db: no existe \"{name}\". Escribe db para ver la lista.");
                1
            }
        };
    }
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|c| vec![c.name.clone(), c.kind.label().to_string(), format!("{GRAY}{}{RESET}", c.target())])
        .collect();
    let headers = ["Conexión", "Tipo", "Destino"];
    if !std::io::stdin().is_terminal() {
        table::print(&headers, &[Align::Left; 3], &rows);
        return 0;
    }
    let mut options = vec![vec!["+ Nueva conexión".to_string(), String::new(), String::new()]];
    options.extend(rows);
    match picker::select_one(&headers, &options) {
        Some(0) => create(&mut list),
        Some(i) => conn_menu(&mut list, i - 1),
        None => 0,
    }
}

fn console(c: &Conn) -> i32 {
    match command(c, &c.database) {
        Ok(mut cmd) => {
            println!("{GRAY}Conectando a {} ({})… para salir: exit o \\q{RESET}", c.name, c.target());
            cmd.status().map_or(1, |s| s.code().unwrap_or(1))
        }
        Err(e) => {
            eprintln!("{RED}db:{RESET} {e}");
            1
        }
    }
}

fn test(c: &Conn) -> bool {
    match query(c, &c.database, "SELECT 1") {
        Ok(_) => {
            println!("{GREEN}✓{RESET} Conexión a {} correcta", c.name);
            true
        }
        Err(e) => {
            eprintln!("{RED}No se pudo conectar a {}:{RESET} {e}", c.name);
            false
        }
    }
}

/// Lista de tablas → elegir una → sus primeras 20 filas.
fn browse(c: &Conn) -> i32 {
    let mut database = c.database.clone();
    if c.kind == Kind::MySql && database.is_empty() {
        // Sin base elegida: primero elegir cuál.
        let (_, rows) = match query(c, "", "SHOW DATABASES") {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{RED}db:{RESET} {e}");
                return 1;
            }
        };
        let rows: Vec<Vec<String>> = rows.into_iter().filter(|r| !["information_schema", "mysql", "performance_schema", "sys"].contains(&r[0].as_str())).collect();
        let Some(i) = picker::select_one(&["Base de datos"], &rows) else { return 0 };
        database = rows[i][0].clone();
    }
    let sql = match c.kind {
        Kind::MySql => "SELECT table_name AS Tabla, table_rows AS Filas_aprox FROM information_schema.tables WHERE table_schema = DATABASE() ORDER BY table_name",
        Kind::Postgres => "SELECT table_schema AS esquema, table_name AS tabla FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY 1, 2",
    };
    let (headers, rows) = match query(c, &database, sql) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{RED}db:{RESET} {e}");
            return 1;
        }
    };
    if rows.is_empty() {
        println!("{GRAY}No hay tablas.{RESET}");
        return 0;
    }
    if !std::io::stdin().is_terminal() {
        print_result(&headers, &rows);
        return 0;
    }
    let head: Vec<&str> = headers.iter().map(String::as_str).collect();
    println!("{GRAY}{} tablas · Enter para ver las primeras 20 filas{RESET}", rows.len());
    let Some(i) = picker::select_one(&head, &rows) else { return 0 };
    let table_ref = match c.kind {
        Kind::MySql => format!("`{}`", rows[i][0].replace('`', "``")),
        Kind::Postgres => format!("\"{}\".\"{}\"", rows[i][0].replace('"', "\"\""), rows[i][1].replace('"', "\"\"")),
    };
    match query(c, &database, &format!("SELECT * FROM {table_ref} LIMIT 20")) {
        Ok((h, r)) => {
            println!("{YELLOW}{table_ref}{RESET} {GRAY}(primeras 20 filas){RESET}");
            print_result(&h, &r);
            0
        }
        Err(e) => {
            eprintln!("{RED}db:{RESET} {e}");
            1
        }
    }
}

fn conn_menu(list: &mut Vec<Conn>, i: usize) -> i32 {
    let actions = ["Abrir consola", "Ver tablas", "Probar conexión", "Cambiar contraseña", "Borrar"];
    let rows: Vec<Vec<String>> = actions.iter().map(|a| vec![a.to_string()]).collect();
    println!("{GRAY}{} · {} · {}{RESET}", list[i].name, list[i].kind.label(), list[i].target());
    match picker::select_one(&["¿Qué quieres hacer?"], &rows) {
        Some(0) => console(&list[i]),
        Some(1) => browse(&list[i]),
        Some(2) => i32::from(!test(&list[i])),
        Some(3) => {
            let Some(pass) = password(&format!("Nueva contraseña de {} (Enter = sin contraseña): ", list[i].user)) else { return 0 };
            match set_password(&list[i].name, &pass) {
                Ok(()) => i32::from(!test(&list[i])),
                Err(e) => {
                    eprintln!("db: no se pudo guardar la contraseña: {e}");
                    1
                }
            }
        }
        Some(4) => {
            if !confirm(&format!("¿Borrar la conexión {}? (la base de datos no se toca)", list[i].name)) {
                return 0;
            }
            let c = list.remove(i);
            delete_password(&c.name);
            match save(list) {
                Ok(()) => {
                    println!("{GREEN}✓{RESET} Conexión {} borrada", c.name);
                    0
                }
                Err(e) => {
                    eprintln!("db: no se pudo guardar: {e}");
                    1
                }
            }
        }
        _ => 0,
    }
}

/// Asistente: tipo, directo o por túnel, host, puerto, usuario, base y contraseña. Prueba al final.
fn create(list: &mut Vec<Conn>) -> i32 {
    let kinds = vec![vec!["MySQL / MariaDB".to_string()], vec!["PostgreSQL".to_string()]];
    let Some(k) = picker::select_one(&["Tipo"], &kinds) else { return 0 };
    let kind = if k == 0 { Kind::MySql } else { Kind::Postgres };

    let tunnels = crate::tunnel::names();
    let mut via = vec![vec!["Directo (sin túnel)".to_string()]];
    via.extend(tunnels.iter().map(|t| vec![format!("Por el túnel {t}")]));
    let tunnel = if tunnels.is_empty() {
        None
    } else {
        println!("{GRAY}¿Cómo se llega? (para la base de AWS detrás del bastión, elige su túnel){RESET}");
        match picker::select_one(&["Conexión"], &via) {
            Some(0) => None,
            Some(i) => Some(tunnels[i - 1].clone()),
            None => return 0,
        }
    };

    let (host, port) = if tunnel.is_some() {
        (String::from("127.0.0.1"), kind.default_port())
    } else {
        let Some(host) = ask("Host: ", "localhost") else { return 0 };
        let Some(port) = ask("Puerto: ", &kind.default_port().to_string()).and_then(|p| p.parse().ok()) else {
            eprintln!("Puerto no válido.");
            return 1;
        };
        (host, port)
    };
    let Some(user) = ask("Usuario: ", kind.default_user()) else { return 0 };
    let database = ask("Base de datos (vacío = elegir después): ", "").unwrap_or_default();
    let Some(pass) = password(&format!("Contraseña de {user} (se guarda en el almacén seguro del sistema): ")) else { return 0 };
    let suggestion = if database.is_empty() { format!("{}-{}", kind.id(), user) } else { database.clone() };
    let Some(name) = ask("Nombre de la conexión: ", &suggestion) else { return 0 };
    if name.contains(char::is_whitespace) || list.iter().any(|c| c.name == name) {
        eprintln!("Nombre no válido o ya usado: {name}");
        return 1;
    }

    if let Err(e) = set_password(&name, &pass) {
        eprintln!("db: no se pudo guardar la contraseña en el almacén seguro: {e}");
        return 1;
    }
    let conn = Conn { name: name.clone(), kind, host, port, user, database, tunnel };
    list.push(conn.clone());
    if let Err(e) = save(list) {
        eprintln!("db: no se pudo guardar: {e}");
        return 1;
    }
    println!("{GREEN}✓{RESET} Conexión {name} guardada: {}", conn.target());
    i32::from(!test(&conn))
}

pub fn names() -> Vec<String> {
    load().into_iter().map(|c| c.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(kind: Kind) -> Conn {
        Conn { name: "crm".into(), kind, host: "localhost".into(), port: kind.default_port(), user: "root".into(), database: "autospot".into(), tunnel: None }
    }

    #[test]
    fn guarda_y_lee_conexiones() {
        let c = Conn { tunnel: Some("bastion-3306".into()), ..conn(Kind::MySql) };
        assert_eq!(Conn::from_json(&c.to_json()), Some(c.clone()));
        assert_eq!(c.target(), "root@túnel bastion-3306/autospot");
        assert_eq!(conn(Kind::Postgres).target(), "root@localhost:5432/autospot");
        assert!(!c.to_json().to_string().to_lowercase().contains("pass"), "la contraseña nunca va al archivo");
    }

    #[test]
    fn arma_los_argumentos_del_cliente() {
        let my = connect_args(&conn(Kind::MySql), "127.0.0.1", 3307, "autospot");
        assert_eq!(my.join(" "), "-h 127.0.0.1 -P 3307 --protocol=TCP --connect-timeout=8 -u root -D autospot");
        let pg = connect_args(&conn(Kind::Postgres), "localhost", 5432, "");
        assert_eq!(pg.join(" "), "-h localhost -p 5432 -U root -d postgres");
    }

    /// Prueba real contra un PostgreSQL en Docker en el puerto 55432 (usa el almacén seguro real).
    /// `cargo test -- --ignored prueba_real_postgres`
    #[test]
    #[ignore]
    fn prueba_real_postgres() {
        let c = Conn { name: "fysh-pg-test".into(), kind: Kind::Postgres, host: "localhost".into(), port: 55432, user: "postgres".into(), database: "postgres".into(), tunnel: None };
        set_password(&c.name, "prueba123").unwrap();
        assert_eq!(get_password(&c.name).as_deref(), Some("prueba123"), "la contraseña vuelve del almacén seguro");
        query(&c, "postgres", "CREATE TABLE IF NOT EXISTS leads (id int, nombre text); TRUNCATE leads; INSERT INTO leads VALUES (1,'Ana'),(2,'Luis')").unwrap();
        let (h, rows) = query(&c, "postgres", "SELECT * FROM leads ORDER BY id").unwrap();
        println!("PG columnas={h:?} filas={rows:?}");
        assert_eq!(h, ["id", "nombre"]);
        assert_eq!(rows[1], ["2", "Luis"]);
        let wrong = Conn { name: "fysh-pg-mal".into(), ..c.clone() };
        set_password(&wrong.name, "incorrecta").unwrap();
        let err = query(&wrong, "postgres", "SELECT 1").unwrap_err();
        println!("PG error con contraseña mala: {err}");
        delete_password(&c.name);
        delete_password(&wrong.name);
        assert!(get_password(&c.name).is_none(), "se borró del almacén");
    }

    #[test]
    fn lee_resultados_separados_por_tab() {
        let (h, r) = parse_tsv("Tabla\tFilas\nleads\t120\nusers\t5\n");
        assert_eq!(h, ["Tabla", "Filas"]);
        assert_eq!(r, vec![vec!["leads".to_string(), "120".into()], vec!["users".into(), "5".into()]]);
    }
}
