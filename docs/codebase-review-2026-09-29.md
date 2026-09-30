# Auditoría de codebase — 29 de septiembre de 2026

Revisión posterior a `79a8a43`, con tres subagentes especializados en dominio y
persistencia, infraestructura y UI, y revisión integrada de los cambios. Este
informe actualiza las cifras y pendientes acumulados de pasadas anteriores.
Los cambios se aplican y publican en `main` mediante commits semánticos. No se
publica una nueva versión de la aplicación como parte de esta auditoría.

## Alcance y tamaño

Se revisaron los archivos de `src` —ahora 135 Rust y dos consultas GraphQL—,
contratos entre capas, bridges nativos, build, scripts, CI, dependencias,
recursos y harness de terminales. La revisión combinó inventario, referencias,
lectura de caminos críticos, regresiones y comprobaciones integradas. En GPUI
se conservaron las fuentes de biblioteca, build, tests, licencia y procedencia.

| Métrica | Inicio | Final |
| --- | --- | --- |
| Contenido versionado del checkout | 14,45 MB | Aproximadamente 9,50 MB |
| Archivos Rust propios | 132 | 135 |
| Líneas Rust propias, incluidos tests | 56.663 | 58.629 |
| Pruebas Rust correctas de la app | 393 | 431 |
| Pruebas del parche nativo, incluidos doctests | 0 | 9 |
| Pruebas de scripts correctas | 7 | 10 |

La reducción neta es de aproximadamente **4,96 MB, un 34,3 % del contenido
versionado inicial**, incluyendo las nuevas regresiones y el parche nativo. El ahorro principal está en ejemplos ajenos a la aplicación. No mide
reducción del ejecutable ni del historial `.git`: esos ejemplos nunca se
empaquetaban en Vibra. El Rust propio crece por correcciones, pruebas y expansión
de líneas para facilitar la lectura. Hay 38 pruebas Rust netas nuevas en la app; una prueba
se trasladó del lector Git al runner compartido y otras se ampliaron.

## Problemas corregidos y evidencia

### Dominio y persistencia

- **Automatizaciones al cruzar hora o medianoche.** Un horario `:59` consultado
  a `:04` desaparecía dentro de la gracia de diez minutos. Se considera el periodo
  anterior y weekdays valida el día de la ocurrencia. Horarios JSON fuera de rango
  se rechazan. La regresión cubre gracia, expiración y ausencia de repetición.
- **UUIDs duplicados.** Notes y Automations resolvían el primer registro; una
  segunda automatización duplicada podía quedar siempre pendiente y repetir
  el primer comando. La normalización repara las identidades sin eliminar
  contenido. Se prueban edición, eliminación, ejecución e idempotencia.
- **Importación de preview concurrente.** Workspace y settings podían reemplazar
  un documento recién guardado por otra instancia. `atomic_write_if_missing`
  toma el mismo lock del guardado y comprueba el destino dentro del lock.
  La regresión coordina escritor e importador y conserva el snapshot nuevo.
- **Resize de panes anidados.** Alcanzar el límite del divisor cercano hacía
  que otra pulsación modificara un ancestro. Se distinguen ausencia de divisor
  y ausencia de cambio. Se prueban ambos ejes, límites y movimiento inverso.

### Git, procesos e integraciones

- **Stage/unstage con pathspecs.** Nombres como `literal*.txt` o `:(glob)*.txt`
  podían afectar otros archivos. Se usa `--literal-pathspecs` y se comprueba
  la conservación de las demás entradas del índice.
- **Unstage antes del primer commit.** Fallaba si el archivo cambió después
  de staging. `git rm --cached -f` retira solo el índice cuando no hay HEAD;
  la prueba verifica que el contenido local se conserva.
- **Git sin deadline y workers bloqueados.** Lecturas y probes de versión tienen
  plazos de 30 y tres segundos. La captura compartida conserva el byte adicional
  de truncamiento y termina los diffs al alcanzar su presupuesto. I/O no
  bloqueante, cancelación y drenado final acotado permiten reunir los workers
  aunque un descendiente separado mantenga una tubería abierta. Se prueban
  stdin bloqueado, tubería sin EOF, consulta infinita, descarte de salida y
  conservación de 256 KiB completos al salir. Las escrituras conservan su política
  de 120 segundos; solo se termina el grupo propio, no procesos ajenos a él.
- **Contexto de commit con patches grandes y textconv.** Un diff superior a
  32 MiB impedía generar un contexto final de 48 KiB. Ahora el patch es truncable
  y desactiva textconv. Se prueban el aviso y la ausencia de ejecución del conversor.
- **Socket de tracking sin timeout.** La CLI podía esperar indefinidamente;
  se aplican timeouts de lectura/escritura. La regresión usa un servidor que
  no responde y conserva el límite de respuesta de 4 MiB.
- **Lecturas ilimitadas de hooks.** Configs y backups tienen límite de 4 MiB;
  scripts, de 64 KiB. Dos regresiones verifican que el rechazo ocurre antes de
  reemplazar datos o generar backups/scripts.
- **Git inconsistente en Inbox.** GitHub usaba `/usr/bin/git` aunque la app
  hubiera descubierto otro binario utilizable. Usa ahora el mismo adaptador.

### UI, texto y temas

- **Panic UTF-8 al borrar palabras.** Alt+Backspace podía truncar dentro de un
  espacio Unicode. Campos y comentarios de revisión comparten `delete_last_word`
  con límites UTF-8. Se prueban tres espacios Unicode y la ruta GPUI del diff.
- **Copy alteraba el archivo.** Conserva ahora el texto original y transforma
  solo las líneas de presentación. La regresión comprueba tabs, BOM y CRLF.
- **Resaltado Rust.** `'a'` y `'_'` se confundían con lifetimes y contaminaban
  líneas posteriores. Dos pruebas distinguen ambas construcciones.
- **Citas Markdown.** La profundidad preserva citas exteriores al cerrar una
  cita anidada, y su estado en headings, código y listas. Dos regresiones lo cubren.
- **Panic de colores Unicode.** `parse_hex` valida ASCII hex antes de cortar
  por posiciones de bytes y rechaza alpha inválido. La regresión incluye `€`.
- **Identidades y pares de temas.** Slugs repetidos ocultaban familias y nombres
  Unicode distintos podían fusionarse como light/dark. Los IDs canónicos se basan
  en el archivo o la identidad del par, independientes de las colisiones. Aliases
  conservan preferencias legacy y migran solo temas disponibles. Se prueban
  extensiones distintas, recargas, colisiones, parejas y nombres Unicode.
- **Límite silencioso de Notes.** Superar 100.000 caracteres conserva ahora
  el texto previo y muestra un error en inglés. La regresión comprueba recuperación.
- **Poda del Inbox.** Se conserva el estado de comentarios y mutaciones pendientes.
  Las conversaciones de terminales cerradas dejan de retener detalles para siempre.
  Una regresión verifica ambos ciclos de vida.

### Scripts

- **Packaging mediante symlink.** `package_app.sh` podía reemplazar un bundle
  externo si `dist` era un enlace. Lo rechaza antes del build; la prueba conserva
  un archivo testigo externo. También limpia temporales al fallar y rechaza
  notarización sin identidad explícita antes de construir.
- **Verificación de sintaxis incompleta.** `zsh -n Scripts/*.sh` analizaba solo
  el primer script. `verify.sh` recorre cada `.sh` y `.zsh`. La regresión introduce
  errores en un script posterior y en el helper `lib.zsh`.

## Limpieza y legibilidad

- Se retiran 30 targets de ejemplos GPUI y sus recursos: 4.869.810 bytes,
  incluido un GIF de 4.471.092 bytes. Se retira su lockfile independiente de
  178.438 bytes; el lockfile raíz sigue fijando el grafo de la aplicación.
  La poda está documentada en `third_party/gpui/VIBRA_PATCHES.md`.
- Se eliminan siete SVG sin referencias y sus entradas de assets: 1.847 bytes.
  Los 71 SVG restantes tienen entradas correspondientes y no faltan archivos.
- `workspaceOrder` deja de reconstruirse y serializarse en schema 7; se conserva
  su lectura para migrar documentos legacy. El fixture comprueba orden e idempotencia.
- Se comparte borrado de palabras y captura Git, y se retiran helpers shell
  sin uso o que solo duplicaban expansión nativa de zsh.
- Prompts largos usan `concat!` conservando su contenido; comandos y condiciones
  de scripts y llamadas Swift se expanden. La pasada final separa callbacks,
  condiciones y valores JSON densos. Rust conserva su formato estándar; no quedan
  líneas de más de 100 caracteres en código Rust propio, bridges nativos ni scripts.
- No se identificó una dependencia directa eliminable con evidencia suficiente.

## Cierre de pendientes de la segunda pasada

| Pendiente anterior | Cambio aplicado |
| --- | --- |
| Scripts POSIX ejecutados con fish/nu | Un adaptador carga el entorno de login y ejecuta el script con `/bin/sh`; transporta el script fuera de la sintaxis del shell exterior. Validación con sh, bash, zsh, fish y Nushell reales, PATH de login, stdin y argumentos literales. |
| Locks Ghostty con panic o error nativo | Estado de fallo permanente y evento `TerminalEvent::Failed`; conserva el último snapshot Rust, rechaza nuevas operaciones nativas y solicita shutdown/reaping. La UI conserva el error tras `Exit` y rechaza entrada y pegados pendientes. |
| Incompatibilidad futura de `block 0.1.6` | Parche local del tipo opaco de `_NSConcreteStackBlock` y ABI C explícita. API y layout conservados; llamadas C, copia al heap y liberación de capturas probadas en arm64 e Intel. |
| IDs de temas dependientes del conjunto | IDs canónicos deterministas y aliases de migración. Carga, agrupación y compatibilidad aisladas en `theme/user.rs`. |
| Workspace leído/parseado dos veces | Una lectura y decode tipado bajo lock, con los mismos bytes para importación y backups. Distingue schema ausente de cero explícito sin un segundo árbol JSON en la ruta normal. |
| Lowercase en cada comparación del heap | Clave calculada una vez por entrada; se conserva orden de directorios, desempate y presupuesto de entradas. |
| Clones completos de library y tabs | Serialización de una vista prestada de library, copiando solo registros que requieran reparar IDs; tabs/titlebar renderizan referencias y canvas copia solo el layout. |
| Builders de UI extensos | Notes separa lista, fila, controles y cuerpo del editor; temas separa catálogo y render de tarjetas. Se conserva el diseño existente. |
| Estado de RevisionGuard en tres mutex | Revisión, recuperación y merge-input se protegen en un solo estado; se mantienen preservación de cambios externos y copias de recuperación. |

La segunda pasada también corrige el uso de caché Sparkle antes de validar versión
y checksum, y distingue un documento ausente de un symlink roto para bloquear
un guardado tras una carga fallida.

La revisión visual corresponde al usuario según `AGENTS.md`; no se utilizó
navegador. Las dos pruebas opt-in requieren un benchmark manual y sesiones reales
de proveedores. No se ejecuta notarización ni publicación de un release.

## Validación final

`./Scripts/verify.sh` pasó completo:

- `cargo fmt --check`.
- `cargo test --locked`: **431 pasan, 0 fallan, 2 ignoradas**.
- Harness nativo `block`: **7 unit tests y 2 doctests pasan**; también se ejecutó
  en `x86_64-apple-darwin` mediante Rosetta, con el mismo resultado.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`.
- `python3 Scripts/test_release.py`: **10 pruebas correctas**.
- `plutil -lint` de Info.plist y entitlements.
- Sintaxis de cada script `.sh` y `.zsh`.

También pasaron `git diff --check`, formato del harness Rust y parsing Swift
del generador de iconos, Package.swift y TerminalReplay. Las dos pruebas ignoradas
mantienen sus motivos: benchmark manual y consulta opt-in de cuotas con sesiones
reales. La biblioteca nativa se compiló como parte de Cargo. `cargo check --locked`
pasó también para `x86_64-apple-darwin`. La suite final incluyó fish 4.9.3 y
Nushell 0.116.0 descargados localmente, sin instalar herramientas globales.
No aparece la incompatibilidad futura de `block` en los builds finales.

## Commits de implementación

Todos usan mensajes semánticos:

- `5dfe8e8 chore(gpui): remove unused examples and standalone lockfile`
- `2c7a346 fix(workspace): preserve snapshots and scheduled automation state`
- `407965e fix(infra): bound Git queries and agent tracking I/O`
- `ded3af0 fix(ui): preserve source text and asynchronous task state`
- `7881fe4 fix(build): validate packaging paths and every shell script`
- `60a7751 fix(deps): preserve the native Blocks ABI on current Rust`
- `9773312 fix(build): validate Sparkle configuration before cache reuse`
- `e853cac refactor(storage): share guarded reads and borrow library snapshots`
- `c95340f fix(shell): preserve login environments for POSIX commands`
- `c2cec49 fix(terminal): preserve failure state and reap failed sessions`
- `297dc13 refactor(ui): stabilize themes and split rendering responsibilities`

El commit de documentación que contiene este informe registra el cierre integrado.
