# btop visual language; braille reserved for data-bearing widgets

oxidone adopts btop's *structural* design language wholesale — rounded box-drawing panels, gradient meters, dense information-per-pixel layout — with a Catppuccin palette (four flavors, TOML-selectable, default Mocha). Braille (U+2800) is used **only to encode data**: completion meters (done ÷ total) and the due-load histogram in v1; activity sparklines later. Braille is never used as pure decoration.

btop earns its braille by packing real quantitative data into sub-character resolution; cargo-culting the *look* with decorative braille would betray that. Reserving braille for data keeps every glyph meaningful and the interface honest. btop provides the *shape*, Catppuccin provides the *palette* — the two are independent layers.

## Consequences

- Every braille widget must degrade gracefully: an ASCII-block fallback flag for terminals/fonts lacking Braille Patterns glyphs, and braille is dropped before text when a terminal is too narrow.
- Stat widgets live *inline* in existing panels (btop-style header meters) in v1; a dedicated/overlay dashboard is deferred until sparklines justify it.
