# Plan de fysh: próximas funciones

Diez funciones agrupadas en 5 fases. Cada fase termina con tests, CI en verde y una versión
publicada, así siempre hay algo usable. Tamaño: **S** (horas), **M** (un día), **L** (varios días).

| Fase | Versión | Funciones | Tamaño |
|---|---|---|---|
| 0. Base técnica ✅ | — | `fysh -c`, salida sin colores fuera de la terminal (el módulo de procesos pasó a la Fase 2) | M |
| 1. Shell completa ✅ | v0.3.0 | Pipes, redirecciones, `&&` `\|\|` `;`, comodines | L |
| 2. Desarrollo diario ✅ | v0.4.0 | `run`, `puertos` + `procesos`, `docker` | M + S + M |
| 3. Datos y servidores ✅ | v0.5.0 | `tunel`, `db` | M + M |
| 4. Comodidad ✅ | v0.6.0 | Ctrl+R difuso, entorno por proyecto, notificaciones | M + M + S |
| 5. APIs ✅ | v0.7.0 | `http` | M |

El commit con el aviso de versión nueva y `actualizar` (sin publicar todavía) sale en la v0.3.0.

---

## Fase 0 · Base técnica (la necesitan varias fases)

- **`fysh -c "<comando>"`**: ejecuta un solo comando y sale. Permite usar comandos propios
  (`dir`, `git status` en tabla…) dentro de pipes: la etapa se corre como `fysh -c "dir"`.
- **Sin colores fuera de la terminal**: cuando la salida va a un pipe o archivo, se quitan los
  códigos ANSI (ya existe `table::strip_ansi`). Así `dir > lista.txt` queda limpio.
- **Módulo de procesos** con la librería `sysinfo`: lista de procesos (nombre, pid, memoria,
  CPU) en Windows/Linux/Mac. Lo usan `puertos`, `procesos` y `tunel`.

## Fase 1 · Shell completa → v0.3.0

**1. Pipes, redirecciones, `&&` / `||` / `;` y comodines** — L
- Qué: `git log | findstr fix`, `dir > lista.txt`, `npm test >> log.txt`, `2> errores.txt`,
  `npm run build && npm start`, `cd x || echo no existe`, `rm *.log`, `cat src/*.rs`.
- Cómo:
  - El parser (`parser.rs`) pasa de lista de palabras a un árbol: lista de *pipelines* unidos
    por `&&`/`||`/`;`; cada pipeline = comandos unidos por `|`; cada comando con sus
    redirecciones (`>`, `>>`, `<`, `2>`, `2>&1`). Comillas y `$VAR` igual que hoy.
  - Ejecución: procesos conectados con `Stdio::piped()`. Los comandos propios en un pipe corren
    como `fysh -c` (Fase 0); con el selector o en la terminal siguen igual que hoy.
  - Comodines `*` `?` `**` con la librería `glob`, solo en palabras sin comillas; si no
    coinciden con nada se deja el texto tal cual (como bash) y avisa.
  - `$?` y el código del prompt toman el último comando del pipeline.
- Pruebas: parser (tabla de casos con comillas, escapes y operadores), ejecución de pipes con
  programas reales (`git`, `findstr`/`grep`), redirección a archivos temporales, comodines.
- Riesgo: es el cambio más grande del núcleo. Se hace con tests primero y con `ins:`/`explica`
  actualizados (hoy les decimos que fysh NO soporta pipes).

## Fase 2 · Desarrollo diario → v0.4.0

**2. `run`: scripts del proyecto** — M
- Qué: en cualquier carpeta, selector con todo lo que se puede ejecutar ahí:
  scripts de `package.json` (con npm/pnpm/yarn según el lockfile), servicios de
  `docker-compose.yml`, objetivos de `Makefile`, comandos de `manage.py` (Django),
  `cargo run/test/build`. `run dev` ejecuta directo sin selector.
- Cómo: detectores por archivo (`serde_json` para package.json; lectura simple de YAML
  para compose con la librería `serde_yaml`). Busca también en la carpeta padre (monorepos
  como autospot-CRM con `frontend/` y `backend/`).
- Pruebas: carpetas de ejemplo con cada tipo de proyecto.

**3. `puertos` y `procesos`** — S
- `puertos`: tabla puerto → proceso (pid, nombre, desde cuándo). Al elegir uno: cerrar el
  proceso (pide confirmación) o abrir `http://localhost:<puerto>` en el navegador.
  `puertos 3000` filtra.
- `procesos [texto]`: tabla de procesos (CPU, memoria) con selector para cerrar.
- Cómo: librería `netstat2` (sockets con su pid, multiplataforma) + módulo de procesos.

**4. `docker`** — M
- Qué: `docker` sin argumentos → tabla de contenedores (nombre, imagen, estado, puertos).
  Al elegir uno: ver logs (`-f`), entrar (`exec -it ... sh/bash`), reiniciar, detener, borrar.
  Si Docker Desktop está apagado, ofrece encenderlo. Con argumentos, docker normal.
- Cómo: `docker ps -a --format json` y los mismos comandos del CLI; sin librería nueva.

## Fase 3 · Datos y servidores → v0.5.0

**5. `tunel`: túneles SSH** — M
- Qué: `tunel` → tabla de túneles guardados (nombre, puerto local → destino, vía qué
  servidor, si está activo). Crear uno con asistente: servidor de salto (selector de `ssh`),
  host y puerto de destino (ej. la base de datos RDS:3306), puerto local (ej. 3307).
  Activar/desactivar con Enter. Caso típico: MySQL de AWS pasando por el bastión.
- Cómo: `ssh -N -L puerto:destino:puerto servidor` como proceso en segundo plano; se guarda
  en `~/.fysh/tuneles.json` con su pid. El prompt muestra `⇄2` si hay túneles activos.

**6. `db`: bases de datos** — M
- Qué: `db` → conexiones guardadas (MySQL/PostgreSQL), con estado. Al elegir: abrir la
  consola (`mysql`/`psql`), ver tablas en tabla de fysh, o conectar a través de un túnel
  (se activa solo si hace falta).
- Contraseñas: **nunca en texto plano**. Se guardan en el almacén seguro del sistema
  (Administrador de credenciales de Windows / llavero de Mac / Secret Service en Linux)
  con la librería `keyring`, y se pasan al cliente por variable de entorno.

## Fase 4 · Comodidad → v0.6.0

**7. Búsqueda difusa** — M
- Ctrl+R: historial con búsqueda difusa ("dock up" encuentra
  `docker compose up -d backend`), en el mismo estilo de selector.
- Ctrl+T: buscar archivos de la carpeta (respetando `.gitignore`) e insertarlos en la línea.
- Cómo: librería `nucleo-matcher` (la del editor Helix) para el puntaje; `ignore` para
  recorrer carpetas respetando `.gitignore`. Atajos con `rustyline` (bindings propios).

**8. Entorno por proyecto** — M
- Al entrar a una carpeta: carga su `.env` (solo en la sesión, se descarga al salir),
  activa el entorno virtual de Python (`.venv`/`venv`) y muestra en el prompt
  ` 20.11` / `🐍 3.12` según el proyecto.
- Seguridad: la primera vez que un `.env` se carga en una carpeta, pregunta y recuerda la
  respuesta (como `direnv allow`), para no cargar variables de repos desconocidos.

**9. Notificaciones** — S
- Si un comando tarda más de 30 s y la ventana no está enfocada, notificación de Windows
  "✓ npm run build terminó (2m 05s)" o "✗ … falló".
- Cómo: librería `notify-rust` (notificaciones nativas en Windows/Mac/Linux).

## Fase 5 · APIs → v0.7.0

**10. `http`** — M
- Qué: `http GET localhost:8000/api/leads`, `http POST url nombre=Ana edad:=30`,
  cabeceras `Authorization:Bearer…`. Respuesta con estado y tiempo, JSON coloreado;
  si es una lista de objetos, en tabla de fysh. `http` sin nada → historial de peticiones
  con selector para repetirlas.
- Cómo: librería `ureq` (cliente HTTP pequeño) + `serde_json`.

---

## Reglas para todas las fases

- Cada función: tests (los que tocan el sistema van en carpetas/registros temporales), CI en
  Windows/Linux/Mac en verde antes del tag (ver `AGENTS.md`), y `help`/autocompletado al día.
- Mismo estilo: tablas, selectores con flechas, mensajes en español, confirmación antes de
  borrar o cerrar algo.
- Las acciones peligrosas (cerrar procesos, borrar contenedores) siempre preguntan.
- Lo que solo existe en un sistema se marca con `#[cfg(...)]` y en los otros muestra un aviso.

## Decisiones

- `db`: solo MySQL/MariaDB y PostgreSQL (decidido).
- `tunel`: los túneles quedan activos al cerrar fysh; se apagan con `tunel` (decidido).
- Notificaciones: 30 s por defecto, configurable con `set FYSH_AVISO_SEGUNDOS=N` (0 = apagadas).
- Publicar: ninguna versión se publica sin confirmación del usuario.
