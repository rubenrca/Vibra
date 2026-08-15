# ADR 0002: Vibra es un viewport de chats, no un harness

## Estado

Aceptado.

## Contexto

Vibra era un multiplexor de terminales que detectaba CLIs de agentes dentro
de un PTY. El producto pasa a una interfaz tipo Codex / Zeron: sidebar de
chats, transcript y composer. Los modelos funcionan mejor en su propio
harness (Claude Code, Codex, Grok Build). Si la app cambia política,
prompt o protocolo, deja de ser ese harness.

## Decisión

1. **Vibra no es un harness.** Es viewport + relay de permisos. Si el TUI
   oficial del vendor haría otra cosa, es un bug de Vibra.
2. **Superficie de producto: chat alojado.** ⌘N / ⌘T / File → nueva sesión
   de agente (Claude, Codex o Grok). No hay flujo de producto para “nueva
   terminal”.
3. **Protocolos solo de fábrica.** Claude: stream-json + control. Codex:
   `codex app-server`. Grok: `grok agent stdio`. Prohibido
   `claude-agent-acp`, `codex-acp` u otro adaptador de terceros.
4. **Permisos pass-through.** Cada petición del wire bloquea y se responde
   tal cual. Vibra no inyecta `bypassPermissions`, `agent-full-access` ni
   auto-approve.
5. **La sesión canónica es la del vendor** (`~/.claude`, `~/.codex`,
   `~/.grok`). Vibra guarda `vendor_session_id` para resume. El transcript
   local es un espejo para pintar.
6. **PTY solo para legado y “Abrir TUI”.** Las sesiones terminal ya
   persistidas siguen abriéndose. Un PTY nuevo solo existe para el TUI
   oficial de la misma sesión del vendor.
7. **Worktrees, sync, daemon y panel de shell quedan fuera** de este
   trabajo.

## Consecuencias

- `SessionSnapshot.surface` distingue `Terminal` (legado) y `Hosted`.
- El layout `PaneLayoutSnapshot::Terminal { id }` no se rediseña: el leaf
  sigue siendo un id de sesión.
- Agentes sin tubo de primer partido no se hospedan en un terminal “para
  que quepan”; quedan fuera del launcher.
- Un fork de GPUI / Zed no es requisito para el cockpit.
