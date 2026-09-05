# Evaluación aislada de motores

Harness macOS de evaluación, sin cambios al motor de Vibra. Resultados y límites en [el informe](../../docs/evaluations/ghostty-vt.md). Requiere Rust, Xcode/Clang, Git y Zig 0.16.0 para compilar. No instala dependencias globalmente.

## Preparación

```sh
git clone https://github.com/ghostty-org/ghostty /tmp/vibra-ghostty-evaluation
git -C /tmp/vibra-ghostty-evaluation checkout 492300cad104195411d12217dd22f1cd05f31376
git clone --branch v1.20.0 --depth 1 https://github.com/migueldeicaza/SwiftTerm /tmp/vibra-swiftterm-evaluation
```

Obtener Zig 0.16.0 de https://ziglang.org/download/0.16.0/ y poner su ejecutable en PATH para esta shell. El tarball `zig-aarch64-macos-0.16.0.tar.xz` usado tuvo SHA256 `b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489`.

Desde el checkout Ghostty, para Apple Silicon:

```sh
zig build -Demit-lib-vt -Demit-xcframework=false -Doptimize=ReleaseFast -Dtarget=aarch64-macos.14.0
```

Opcional, compilación Intel y archivo universal (no prueba ejecución Intel):

```sh
zig build -Demit-lib-vt -Demit-xcframework=false -Doptimize=ReleaseFast -Dtarget=x86_64-macos.14.0 --prefix /tmp/vibra-ghostty-x86_64
lipo -create zig-out/lib/libghostty-vt.a /tmp/vibra-ghostty-x86_64/lib/libghostty-vt.a -output /tmp/vibra-ghostty-universal.a
```

## Ejecutar desde la raíz de Vibra

```sh
mkdir -p /tmp/vibra-terminal-fixtures
export GHOSTTY_SOURCE=/tmp/vibra-ghostty-evaluation
export CARGO_TARGET_DIR=/tmp/vibra-terminal-eval-target
export EVAL_FIXTURE_DIR=/tmp/vibra-terminal-fixtures
cargo run --release --locked --manifest-path tools/terminal-eval/Cargo.toml
SWIFTTERM_SOURCE=/tmp/vibra-swiftterm-evaluation swift run --package-path tools/terminal-eval/swift-replay --scratch-path /tmp/vibra-swift-replay-build TerminalReplay /tmp/vibra-terminal-fixtures
cargo fmt --manifest-path tools/terminal-eval/Cargo.toml --check
cargo clippy --locked --manifest-path tools/terminal-eval/Cargo.toml -- -D warnings
otool -L /tmp/vibra-terminal-eval-target/release/vibra-terminal-eval
```

`-- --fixtures-only` omite el benchmark. En Mac Intel, compilar primero la biblioteca para x86_64 y definir `GHOSTTY_LIB_DIR=/tmp/vibra-ghostty-x86_64/lib`. El bridge enlaza explícitamente el archivo `.a` para evitar cargar el dylib vecino.

Ejemplo upstream de snapshots binarios, independiente del replay SwiftTerm:

```sh
clang -O2 -I "$GHOSTTY_SOURCE/include" "$GHOSTTY_SOURCE/example/c-vt-snapshot/src/main.c" "$GHOSTTY_SOURCE/zig-out/lib/libghostty-vt.a" -o /tmp/vibra-ghostty-snapshot-example
/tmp/vibra-ghostty-snapshot-example
```

El harness consume bytes en memoria: no implementa PTY, transporte ni interfaz gráfica. El fingerprint fuerza lectura del render-state pero no compara igualdad semántica entre motores ni mide dibujo. Las pruebas SwiftTerm ejercitan su núcleo sin crear ventanas; no usar `SWIFTTERM_EXCLUDE_APPLE=1` con esta versión en macOS.
