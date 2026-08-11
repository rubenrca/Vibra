# ADR 0001: no incrustar un browser nativo todavía

## Estado

Aceptado por ahora.

## Contexto

Vibra usa GPUI 0.2.2, cuya API pública no ofrece un elemento estable para
componer un `NSView` arbitrario (por ejemplo `WKWebView`) dentro del árbol de
layout. Introducir otra event loop mediante `wry`, o mantener un fork privado de
GPUI, elevaría mucho el riesgo de foco, IME, accesibilidad y ciclo de vida de
ventanas.

## Decisión

Los enlaces seguros siguen abriéndose en el browser del sistema. No se incorpora
un webview embebido hasta que GPUI exponga composición nativa soportada o exista
una integración aislada con pruebas de foco, teclado, navegación y proceso web.

## Consecuencias

- Se evita una dependencia frágil y específica de macOS dentro del núcleo.
- Browser panes no forman parte del producto actual.
- La decisión se revisará al actualizar GPUI; el modelo de panes debe admitir un
  nuevo tipo de contenido sin acoplarlo al backend web elegido.
