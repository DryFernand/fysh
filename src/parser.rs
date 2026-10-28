//! Convierte una línea en comandos.
//! Soporta: espacios, "comillas dobles" (expanden $VAR), 'comillas simples' (literales),
//! pipes `|`, `&&`, `||`, `;`, redirecciones `>`, `>>`, `<`, `2>`, `2>>`, `2>&1` y comodines `*` `?`.
//! Las variables y los comodines se expanden al ejecutar cada comando (no al leer la línea),
//! así `cd src && dir *.rs` busca dentro de src y `false || echo $?` muestra 1.

use crate::Shell;
use std::collections::BTreeMap;

/// Pedazo de una palabra: texto fijo o una variable ($NOMBRE, $?) que se resuelve al ejecutar.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Word {
    pub parts: Vec<Part>,
    /// Tenía comillas (entonces no es alias ni comodín).
    pub quoted: bool,
    /// Tiene * o ? fuera de comillas.
    pub glob: bool,
}

impl Word {
    /// El texto si la palabra es solo texto sin comillas (para reconocer alias).
    fn plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [Part::Text(t)] if !self.quoted => Some(t),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(Word),
    Pipe,
    And,
    Or,
    Seq,
    /// `>` / `>>` (fd 1) o `2>` / `2>>` (fd 2)
    Out { fd: u8, append: bool },
    /// `<`
    In,
    /// `2>&1`
    ErrToOut,
}

/// Cómo se une un pipeline con el anterior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Primero de la línea o después de `;`: siempre se ejecuta.
    Seq,
    /// `&&`: solo si el anterior terminó bien.
    And,
    /// `||`: solo si el anterior falló.
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sink {
    File { path: Word, append: bool },
    /// `2>&1`: los errores van a donde va la salida normal.
    Stdout,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Command {
    pub words: Vec<Word>,
    pub stdin: Option<Word>,
    pub stdout: Option<Sink>,
    pub stderr: Option<Sink>,
}

impl Command {
    pub fn has_redirects(&self) -> bool {
        self.stdin.is_some() || self.stdout.is_some() || self.stderr.is_some()
    }
}

/// Comandos unidos por `|`.
pub type Pipeline = Vec<Command>;

/// Arma una palabra carácter por carácter.
#[derive(Default)]
struct Builder {
    word: Word,
    text: String,
    active: bool,
}

impl Builder {
    fn push(&mut self, c: char) {
        self.active = true;
        self.text.push(c);
    }

    fn var(&mut self, name: String) {
        self.active = true;
        if !self.text.is_empty() {
            self.word.parts.push(Part::Text(std::mem::take(&mut self.text)));
        }
        self.word.parts.push(Part::Var(name));
    }

    fn take(&mut self) -> Option<Word> {
        if !self.active {
            return None;
        }
        if !self.text.is_empty() || self.word.parts.is_empty() {
            self.word.parts.push(Part::Text(std::mem::take(&mut self.text)));
        }
        self.active = false;
        Some(std::mem::take(&mut self.word))
    }

    /// ¿Lo escrito hasta ahora es exactamente "2" sin comillas? (para `2>`)
    fn is_fd2(&self) -> bool {
        self.active && !self.word.quoted && self.word.parts.is_empty() && self.text == "2"
    }
}

/// Lee el nombre después de '$'. "$?" = código del último comando; "$" solo es un '$'.
fn read_var(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    if chars.peek() == Some(&'?') {
        chars.next();
        return Some("?".into());
    }
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_alphanumeric() || c == '_' {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    (!name.is_empty()).then_some(name)
}

fn tokenize(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut b = Builder::default();
    // BOM: marca invisible que agregan PowerShell y Notepad al inicio del texto.
    let mut chars = line.trim_start_matches('\u{feff}').chars().peekable();
    let flush = |b: &mut Builder, tokens: &mut Vec<Token>| {
        if let Some(w) = b.take() {
            tokens.push(Token::Word(w));
        }
    };

    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => flush(&mut b, &mut tokens),
            '\'' => {
                b.active = true;
                b.word.quoted = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    b.text.push(c);
                }
            }
            '"' => {
                b.active = true;
                b.word.quoted = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '$' => match read_var(&mut chars) {
                            Some(name) => b.var(name),
                            None => b.text.push('$'),
                        },
                        _ => b.text.push(c),
                    }
                }
            }
            '$' => match read_var(&mut chars) {
                Some(name) => b.var(name),
                None => b.push('$'),
            },
            '|' => {
                flush(&mut b, &mut tokens);
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Or);
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                flush(&mut b, &mut tokens);
                tokens.push(Token::And);
            }
            ';' => {
                flush(&mut b, &mut tokens);
                tokens.push(Token::Seq);
            }
            '<' => {
                flush(&mut b, &mut tokens);
                tokens.push(Token::In);
            }
            '>' => {
                let fd = if b.is_fd2() {
                    b = Builder::default();
                    2
                } else {
                    flush(&mut b, &mut tokens);
                    1
                };
                let append = chars.peek() == Some(&'>');
                if append {
                    chars.next();
                }
                if fd == 2 && !append && chars.peek() == Some(&'&') {
                    let mut look = chars.clone();
                    look.next();
                    if look.next() == Some('1') {
                        chars.next();
                        chars.next();
                        tokens.push(Token::ErrToOut);
                        continue;
                    }
                }
                tokens.push(Token::Out { fd, append });
            }
            '*' | '?' => {
                b.word.glob = true;
                b.push(c);
            }
            _ => b.push(c),
        }
    }
    flush(&mut b, &mut tokens);
    tokens
}

/// Reemplaza los alias que aparecen como comando (al inicio o después de | && || ;).
/// El valor del alias puede traer sus propios operadores. No se expanden alias dentro de alias.
fn expand_aliases(tokens: Vec<Token>, aliases: &BTreeMap<String, String>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut at_command = true;
    let mut after_redirect = false;
    for t in tokens {
        match t {
            Token::Word(w) => {
                let alias = (at_command && !after_redirect).then(|| w.plain().and_then(|n| aliases.get(n))).flatten();
                match alias {
                    Some(value) => out.extend(tokenize(value)),
                    None => out.push(Token::Word(w)),
                }
                if !after_redirect {
                    at_command = false;
                }
                after_redirect = false;
            }
            Token::Pipe | Token::And | Token::Or | Token::Seq => {
                at_command = true;
                out.push(t);
            }
            Token::Out { .. } | Token::In => {
                after_redirect = true;
                out.push(t);
            }
            Token::ErrToOut => out.push(t),
        }
    }
    out
}

fn symbol(t: &Token) -> &'static str {
    match t {
        Token::Pipe => "|",
        Token::And => "&&",
        Token::Or => "||",
        Token::Seq => ";",
        Token::Out { fd: 2, append: true } => "2>>",
        Token::Out { fd: 2, .. } => "2>",
        Token::Out { append: true, .. } => ">>",
        Token::Out { .. } => ">",
        Token::In => "<",
        Token::ErrToOut => "2>&1",
        Token::Word(_) => "palabra",
    }
}

/// Línea → lista de (cómo se une, pipeline). Error con un mensaje en español si está mal escrita.
pub fn parse_line(line: &str, aliases: &BTreeMap<String, String>) -> Result<Vec<(Op, Pipeline)>, String> {
    let tokens = expand_aliases(tokenize(line), aliases);
    let mut result: Vec<(Op, Pipeline)> = Vec::new();
    let mut op = Op::Seq;
    let mut pipeline: Pipeline = Vec::new();
    let mut cmd = Command::default();
    let mut it = tokens.into_iter().peekable();

    let syntax = |near: &str| format!("fysh: error de sintaxis cerca de '{near}'");
    while let Some(t) = it.next() {
        match t {
            Token::Word(w) => cmd.words.push(w),
            Token::Out { .. } | Token::In => {
                let Some(Token::Word(target)) = it.next() else {
                    return Err(format!("fysh: falta el archivo después de '{}'", symbol(&t)));
                };
                match t {
                    Token::In => cmd.stdin = Some(target),
                    Token::Out { fd: 1, append } => cmd.stdout = Some(Sink::File { path: target, append }),
                    Token::Out { append, .. } => cmd.stderr = Some(Sink::File { path: target, append }),
                    _ => {}
                }
            }
            Token::ErrToOut => cmd.stderr = Some(Sink::Stdout),
            Token::Pipe => {
                if cmd.words.is_empty() {
                    return Err(syntax("|"));
                }
                pipeline.push(std::mem::take(&mut cmd));
            }
            Token::And | Token::Or | Token::Seq => {
                if cmd.words.is_empty() {
                    // ";" al final o ";;" no son error; un operador sin comando antes sí.
                    if t == Token::Seq && pipeline.is_empty() {
                        continue;
                    }
                    return Err(syntax(symbol(&t)));
                }
                pipeline.push(std::mem::take(&mut cmd));
                result.push((op, std::mem::take(&mut pipeline)));
                op = match t {
                    Token::And => Op::And,
                    Token::Or => Op::Or,
                    _ => Op::Seq,
                };
                if op != Op::Seq && it.peek().is_none() {
                    return Err(syntax(symbol(&t)));
                }
            }
        }
    }
    if !cmd.words.is_empty() {
        pipeline.push(cmd);
    } else if !pipeline.is_empty() {
        return Err(syntax("|"));
    } else if cmd.has_redirects() {
        return Err("fysh: falta el comando".into());
    }
    if !pipeline.is_empty() {
        result.push((op, pipeline));
    }
    Ok(result)
}

/// Texto final de una palabra, con sus variables resueltas ahora.
pub fn expand_word(word: &Word, shell: &Shell) -> String {
    word.parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.clone(),
            Part::Var(n) if n == "?" => shell.last_status.to_string(),
            Part::Var(n) => std::env::var(n).unwrap_or_default(),
        })
        .collect()
}

/// Argumentos finales: variables resueltas y comodines expandidos a los archivos que coinciden
/// (ordenados). Si un comodín no coincide con nada queda tal cual, como en bash.
pub fn expand_args(words: &[Word], shell: &Shell) -> Vec<String> {
    let mut args = Vec::new();
    for w in words {
        let text = expand_word(w, shell);
        if w.glob {
            let mut found: Vec<String> = glob::glob(&text)
                .map(|paths| paths.filter_map(Result::ok).map(|p| p.display().to_string()).collect())
                .unwrap_or_default();
            if !found.is_empty() {
                found.sort();
                args.extend(found);
                continue;
            }
        }
        args.push(text);
    }
    args
}

/// Palabras del primer comando de la línea, ya resueltas (sin comodines).
/// Para mirar rápido qué comando es (por ejemplo, `history`).
pub fn parse(line: &str, shell: &Shell) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .map_while(|t| match t {
            Token::Word(w) => Some(expand_word(&w, shell)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Shell {
        let mut sh = Shell::new();
        sh.last_status = 3;
        sh
    }

    fn words(cmd: &Command, sh: &Shell) -> Vec<String> {
        cmd.words.iter().map(|w| expand_word(w, sh)).collect()
    }

    #[test]
    fn parsing() {
        let sh = shell();
        std::env::set_var("FYSH_T", "hola");
        assert_eq!(parse("  ls   -la  ", &sh), ["ls", "-la"]);
        assert_eq!(parse(r#"echo "a b" 'c $X'"#, &sh), ["echo", "a b", "c $X"]);
        assert_eq!(parse(r#"echo $FYSH_T "$FYSH_T!" $?"#, &sh), ["echo", "hola", "hola!", "3"]);
        assert_eq!(parse("echo ''", &sh), ["echo", ""]);
        assert_eq!(parse("\u{feff}echo hi", &sh), ["echo", "hi"]);
        assert_eq!(parse("echo 100$ a&b", &sh), ["echo", "100$", "a&b"]);
    }

    #[test]
    fn operadores_y_pipes() {
        let sh = shell();
        let none = BTreeMap::new();
        let p = parse_line("git log | findstr fix && echo ok || echo mal ; dir", &none).unwrap();
        let ops: Vec<Op> = p.iter().map(|(op, _)| *op).collect();
        assert_eq!(ops, [Op::Seq, Op::And, Op::Or, Op::Seq]);
        assert_eq!(p[0].1.len(), 2);
        assert_eq!(words(&p[0].1[1], &sh), ["findstr", "fix"]);
        assert_eq!(words(&p[3].1[0], &sh), ["dir"]);
        // Operadores dentro de comillas son texto.
        let q = parse_line("echo \"a | b && c\" 'x;y'", &none).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(words(&q[0].1[0], &sh), ["echo", "a | b && c", "x;y"]);
    }

    #[test]
    fn redirecciones() {
        let sh = shell();
        let none = BTreeMap::new();
        let p = parse_line("cargo build >> log.txt 2>&1 < entrada.txt", &none).unwrap();
        let c = &p[0].1[0];
        assert_eq!(words(c, &sh), ["cargo", "build"]);
        assert!(matches!(&c.stdout, Some(Sink::File { path, append: true }) if expand_word(path, &sh) == "log.txt"));
        assert_eq!(c.stderr, Some(Sink::Stdout));
        assert_eq!(expand_word(c.stdin.as_ref().unwrap(), &sh), "entrada.txt");

        let e = parse_line("npm test 2> errores.txt>salida.txt", &none).unwrap();
        let c = &e[0].1[0];
        assert!(matches!(&c.stderr, Some(Sink::File { path, append: false }) if expand_word(path, &sh) == "errores.txt"));
        assert!(matches!(&c.stdout, Some(Sink::File { path, .. }) if expand_word(path, &sh) == "salida.txt"));
        // "a2>x" no es redirección de errores: es la palabra "a2" y luego > x
        let f = parse_line("echo a2>x", &none).unwrap();
        assert_eq!(words(&f[0].1[0], &sh), ["echo", "a2"]);
    }

    #[test]
    fn errores_de_sintaxis() {
        let none = BTreeMap::new();
        assert!(parse_line("| dir", &none).is_err());
        assert!(parse_line("dir |", &none).is_err());
        assert!(parse_line("dir &&", &none).is_err());
        assert!(parse_line("&& dir", &none).is_err());
        assert!(parse_line("dir >", &none).is_err());
        assert!(parse_line("dir ; ", &none).is_ok());
        assert!(parse_line("", &none).unwrap().is_empty());
    }

    #[test]
    fn variables_al_ejecutar_y_comodines() {
        let mut sh = shell();
        let none = BTreeMap::new();
        let p = parse_line("echo $? *.rs '*.md' src/?ain.rs", &none).unwrap();
        let w = &p[0].1[0].words;
        sh.last_status = 7; // $? se lee al ejecutar, no al leer la línea
        assert_eq!(expand_word(&w[1], &sh), "7");
        assert!(w[2].glob && !w[3].glob && w[4].glob);
        assert_eq!(expand_args(&w[3..4], &sh), ["*.md"]);
        // Sin coincidencias, el comodín queda tal cual.
        let nada = parse_line("x *.no-existe-fysh", &none).unwrap();
        assert_eq!(expand_args(&nada[0].1[0].words[1..], &sh), ["*.no-existe-fysh"]);
    }

    #[test]
    fn alias_en_cualquier_comando() {
        let sh = shell();
        let mut aliases = BTreeMap::new();
        aliases.insert("gl".to_string(), "git log --oneline".to_string());
        aliases.insert("ok".to_string(), "echo bien && echo listo".to_string());
        let p = parse_line("gl -5 | findstr gl && ok", &aliases).unwrap();
        assert_eq!(words(&p[0].1[0], &sh), ["git", "log", "--oneline", "-5"]);
        assert_eq!(words(&p[0].1[1], &sh), ["findstr", "gl"], "gl como argumento no se expande");
        assert_eq!(p.len(), 3, "el alias trae su propio &&");
        assert_eq!(words(&p[2].1[0], &sh), ["echo", "listo"]);
        let q = parse_line("'gl'", &aliases).unwrap();
        assert_eq!(words(&q[0].1[0], &sh), ["gl"], "con comillas no es alias");
    }
}
