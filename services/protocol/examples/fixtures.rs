use vibra_remote_protocol::*;
fn main() {
    let pane_id = uuid::Uuid::nil();
    let messages = [
        Message::ListPanes {},
        Message::Open {
            pane_id,
            size: Size {
                columns: 80,
                rows: 24,
            },
        },
        Message::Screen {
            pane_id,
            revision: 10,
            size: Size {
                columns: 80,
                rows: 24,
            },
            ansi: "\x1b[31mEspañol 日本語 🦀\x1b[0m".into(),
        },
        Message::Input {
            pane_id,
            input: Input::Key {
                key: Key::Character('c'),
                modifiers: vec![Modifier::Control],
            },
        },
    ];
    for (i, message) in messages.into_iter().enumerate() {
        println!(
            "{}",
            String::from_utf8(Envelope::new(i as u64, message).encode().unwrap()).unwrap()
        );
    }
}
