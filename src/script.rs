//! Bloques de script, al estilo de fish:
//!   if cond; …; else if cond; …; else; …; end
//!   for x in a b *.txt $(comando); …; end
//!   while cond; …; end
//!   function nombre; …($1, $argv)…; end
//! más break, continue y return. Se arman sobre los pasos del parser: `if`, `else`, `end`… son la
//! primera palabra de un paso, así que se separan con `;` o con saltos de línea.

use crate::parser::{self, Op, Step, Word};
use crate::Shell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Se pone en true con Ctrl+C: corta bucles y scripts (los programas reciben su propio Ctrl+C).
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// break / continue / return pendientes mientras se sale de los bloques.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Flow {
    #[default]
    Normal,
    Break,
    Continue,
    Return,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Run(Step),
    If { op: Op, branches: Vec<(Vec<Node>, Vec<Node>)>, otherwise: Vec<Node> },
    For { op: Op, var: String, items: Vec<Word>, body: Vec<Node> },
    While { op: Op, cond: Vec<Node>, body: Vec<Node> },
    Function { op: Op, name: String, body: Vec<Node> },
}

impl Node {
    fn op(&self) -> Op {
        match self {
            Node::Run(s) => s.op,
            Node::If { op, .. } | Node::For { op, .. } | Node::While { op, .. } | Node::Function { op, .. } => *op,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// Falta el `end` de un bloque: en el prompt, Enter agrega otra línea.
    Incomplete,
    Error(String),
}

fn error(msg: &str) -> ParseError {
    ParseError::Error(format!("fysh: {msg}"))
}

/// Cómo terminó un bloque.
enum End {
    Eof,
    End,
    Else,
}

fn keyword(step: &Step) -> Option<&str> {
    step.pipeline.first()?.words.first()?.plain()
}

/// Quita la palabra clave del paso. true si queda algo después.
fn strip_keyword(step: &mut Step) -> bool {
    step.pipeline[0].words.remove(0);
    !step.pipeline[0].words.is_empty()
}

struct BlockParser {
    steps: Vec<Step>,
    pos: usize,
}

impl BlockParser {
    fn block(&mut self, inside: bool) -> Result<(Vec<Node>, End), ParseError> {
        let mut nodes = Vec::new();
        while self.pos < self.steps.len() {
            let step = &self.steps[self.pos];
            let alone = step.pipeline.len() == 1 && step.pipeline[0].words.len() == 1;
            match keyword(step) {
                Some("end") if alone => {
                    if !inside {
                        return Err(error("'end' sin un bloque que cerrar"));
                    }
                    self.pos += 1;
                    return Ok((nodes, End::End));
                }
                Some("else") => {
                    if !inside {
                        return Err(error("'else' sin un if antes"));
                    }
                    return Ok((nodes, End::Else)); // parse_if lo lee desde aquí
                }
                Some("if") => nodes.push(self.parse_if()?),
                Some("while") => {
                    let (op, cond) = self.condition("while")?;
                    nodes.push(Node::While { op, cond, body: self.body()? });
                }
                Some("for") => nodes.push(self.parse_for()?),
                Some("function") => nodes.push(self.parse_function()?),
                _ => {
                    nodes.push(Node::Run(step.clone()));
                    self.pos += 1;
                }
            }
        }
        if inside {
            Err(ParseError::Incomplete)
        } else {
            Ok((nodes, End::Eof))
        }
    }

    /// Cuerpo de for/while/function: hasta su `end`.
    fn body(&mut self) -> Result<Vec<Node>, ParseError> {
        match self.block(true)? {
            (nodes, End::End) => Ok(nodes),
            _ => Err(error("'else' sin un if antes")),
        }
    }

    /// La condición de if/while: el paso actual sin la palabra clave, más los que siguen unidos
    /// por && o ||. Devuelve también cómo se une el bloque con lo anterior.
    fn condition(&mut self, what: &str) -> Result<(Op, Vec<Node>), ParseError> {
        let step = &mut self.steps[self.pos];
        let op = step.op;
        if !strip_keyword(step) {
            return Err(error(&format!("falta la condición después de '{what}'")));
        }
        step.op = Op::Seq;
        let mut cond = vec![Node::Run(step.clone())];
        self.pos += 1;
        while let Some(next) = self.steps.get(self.pos).filter(|s| s.op != Op::Seq) {
            cond.push(Node::Run(next.clone()));
            self.pos += 1;
        }
        Ok((op, cond))
    }

    fn parse_if(&mut self) -> Result<Node, ParseError> {
        let (op, mut cond) = self.condition("if")?;
        let mut branches = Vec::new();
        loop {
            let (body, end) = self.block(true)?;
            branches.push((cond, body));
            if !matches!(end, End::Else) {
                return Ok(Node::If { op, branches, otherwise: Vec::new() });
            }
            // `else if cond`, `else` solo, o `else comando`.
            let step = &mut self.steps[self.pos];
            let rest = strip_keyword(step);
            if rest && keyword(step) == Some("if") {
                cond = self.condition("else if")?.1;
                continue;
            }
            if rest {
                step.op = Op::Seq;
            } else {
                self.pos += 1;
            }
            let otherwise = self.body()?;
            return Ok(Node::If { op, branches, otherwise });
        }
    }

    fn parse_for(&mut self) -> Result<Node, ParseError> {
        let step = &self.steps[self.pos];
        let words = &step.pipeline[0].words;
        let var = words.get(1).and_then(Word::plain).map(String::from);
        let has_in = words.get(2).and_then(Word::plain) == Some("in");
        let (Some(var), true, 1) = (var, has_in, step.pipeline.len()) else {
            return Err(error("se escribe: for nombre in a b c; …; end"));
        };
        let (op, items) = (step.op, words[3..].to_vec());
        self.pos += 1;
        Ok(Node::For { op, var, items, body: self.body()? })
    }

    fn parse_function(&mut self) -> Result<Node, ParseError> {
        let step = &self.steps[self.pos];
        let words = &step.pipeline[0].words;
        let name = words.get(1).and_then(Word::plain).map(String::from).filter(|_| words.len() == 2);
        let Some(name) = name else {
            return Err(error("se escribe: function nombre; …; end"));
        };
        let op = step.op;
        self.pos += 1;
        Ok(Node::Function { op, name, body: self.body()? })
    }
}

pub fn parse(steps: Vec<Step>) -> Result<Vec<Node>, ParseError> {
    BlockParser { steps, pos: 0 }.block(false).map(|(nodes, _)| nodes)
}

/// ¿Falta el `end` de algún bloque? (para pedir más líneas en el prompt o en un archivo).
pub fn incomplete(text: &str) -> bool {
    parser::parse_line(text, &BTreeMap::new()).is_ok_and(|steps| parse(steps) == Err(ParseError::Incomplete))
}

/// Ejecuta nodos en orden respetando && / || y devuelve el código del último.
pub fn run(shell: &mut Shell, nodes: &[Node]) -> i32 {
    let mut status = 0;
    for node in nodes {
        match node.op() {
            Op::And if status != 0 => continue,
            Op::Or if status == 0 => continue,
            _ => {}
        }
        status = run_node(shell, node);
        shell.last_status = status; // el siguiente comando ve este $?
        if !shell.running || shell.flow != Flow::Normal || interrupted() {
            break;
        }
    }
    status
}

fn run_node(shell: &mut Shell, node: &Node) -> i32 {
    match node {
        Node::Run(step) => crate::exec::run_step(shell, step),
        Node::If { branches, otherwise, .. } => {
            for (cond, body) in branches {
                let ok = run(shell, cond) == 0;
                if shell.flow != Flow::Normal || interrupted() {
                    return shell.last_status;
                }
                if ok {
                    return run(shell, body);
                }
            }
            run(shell, otherwise)
        }
        Node::For { var, items, body, .. } => {
            let mut status = 0;
            for value in parser::expand_args(items, shell) {
                std::env::set_var(var, value);
                status = run(shell, body);
                if stop_loop(shell) {
                    break;
                }
            }
            status
        }
        Node::While { cond, body, .. } => {
            let mut status = 0;
            loop {
                if run(shell, cond) != 0 || shell.flow != Flow::Normal || interrupted() {
                    break;
                }
                status = run(shell, body);
                if stop_loop(shell) {
                    break;
                }
            }
            status
        }
        Node::Function { name, body, .. } => {
            shell.functions.insert(name.clone(), body.clone());
            0
        }
    }
}

/// Después de cada vuelta: break corta, continue sigue, return y Ctrl+C cortan.
fn stop_loop(shell: &mut Shell) -> bool {
    match shell.flow {
        Flow::Break => {
            shell.flow = Flow::Normal;
            true
        }
        Flow::Continue => {
            shell.flow = Flow::Normal;
            false
        }
        Flow::Return => true,
        Flow::Normal => interrupted() || !shell.running,
    }
}

/// Llama una función: sus argumentos son $1, $2… y $argv mientras corre.
pub fn call(shell: &mut Shell, body: &[Node], args: &[String]) -> i32 {
    let saved = std::mem::replace(&mut shell.argv, args.to_vec());
    let status = run(shell, body);
    shell.argv = saved;
    shell.flow = Flow::Normal; // return termina aquí; break/continue no salen de la función
    status
}

/// Ejecuta un texto de varias líneas (~/.fyshrc o un archivo .fysh): cada línea, o cada bloque
/// completo (if … end), como si se escribiera en el prompt.
pub fn run_text(shell: &mut Shell, text: &str) {
    let mut chunk = String::new();
    for line in text.lines() {
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(line);
        if chunk.trim().is_empty() || incomplete(&chunk) {
            continue;
        }
        crate::execute(shell, &chunk);
        chunk.clear();
        if !shell.running || shell.flow == Flow::Return || interrupted() {
            break;
        }
    }
    if !chunk.trim().is_empty() && shell.running {
        crate::execute(shell, &chunk); // un bloque sin `end`: que avise el error
    }
    shell.flow = Flow::Normal;
}

/// `source archivo [args]` o `fysh archivo.fysh [args]`: ejecuta el archivo en esta shell.
pub fn run_file(shell: &mut Shell, path: &str, args: &[String]) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fysh: {path}: {}", crate::builtins::error_msg(&e));
            return 1;
        }
    };
    let saved = std::mem::replace(&mut shell.argv, args.to_vec());
    run_text(shell, &text);
    shell.argv = saved;
    shell.last_status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(text: &str) -> Result<Vec<Node>, ParseError> {
        parse(parser::parse_line(text, &BTreeMap::new()).unwrap())
    }

    #[test]
    fn arma_los_bloques() {
        let n = blocks("if test -f a && test -f b; echo si; else if test -d c; echo dir; else; echo no; end; echo fin").unwrap();
        assert_eq!(n.len(), 2);
        let Node::If { branches, otherwise, .. } = &n[0] else { panic!() };
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].0.len(), 2, "la condición incluye el &&");
        assert_eq!(otherwise.len(), 1);

        let f = blocks("for x in a b c\n  echo $x\n  if test $x = b; break; end\nend").unwrap();
        let Node::For { var, items, body, .. } = &f[0] else { panic!() };
        assert_eq!((var.as_str(), items.len(), body.len()), ("x", 3, 2));

        let w = blocks("function saluda; echo hola $1; end; while false; end").unwrap();
        assert!(matches!(&w[0], Node::Function { name, .. } if name == "saluda"));
        assert!(matches!(&w[1], Node::While { body, .. } if body.is_empty()));
        // `else comando` en la misma línea
        let e = blocks("if false; echo a; else echo b; end").unwrap();
        assert!(matches!(&e[0], Node::If { otherwise, .. } if otherwise.len() == 1));
    }

    #[test]
    fn bloques_incompletos_y_errores() {
        assert!(incomplete("if true"));
        assert!(incomplete("for x in a b; echo $x"));
        assert!(incomplete("function f\n  if true; echo"));
        assert!(!incomplete("if true; echo; end"));
        assert!(!incomplete("echo end"), "end como argumento no cierra nada");
        assert!(matches!(blocks("end"), Err(ParseError::Error(_))));
        assert!(matches!(blocks("else"), Err(ParseError::Error(_))));
        assert!(matches!(blocks("for x a b; end"), Err(ParseError::Error(_))));
        assert!(matches!(blocks("if; end"), Err(ParseError::Error(_))));
        assert!(matches!(blocks("while true; else; end"), Err(ParseError::Error(_))));
    }
}
