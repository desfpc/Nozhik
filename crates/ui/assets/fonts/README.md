# Embedded fonts

JetBrains Mono Regular is used for documentation popups and terminal text.
Source: https://github.com/JetBrains/JetBrainsMono
Font: https://raw.githubusercontent.com/JetBrains/JetBrainsMono/master/fonts/ttf/JetBrainsMono-Regular.ttf
License: SIL OFL 1.1, preserved in OFL.txt.

Noto Sans Symbols 2 supplies terminal Braille patterns, symbols and legacy
computing glyphs. The original font is embedded without modification.
Font: https://raw.githubusercontent.com/notofonts/noto-fonts/main/hinted/ttf/NotoSansSymbols2/NotoSansSymbols2-Regular.ttf
License: SIL OFL 1.1, preserved in NotoSansSymbols2-OFL.txt.

Noto Emoji supplies modern monochrome emoji. The original variable font is
embedded without modification, at its default weight.
Font: https://raw.githubusercontent.com/google/fonts/main/ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf
License: SIL OFL 1.1, preserved in NotoEmoji-OFL.txt.

The dedicated Terminal family retains egui's stock fallbacks after these fonts.
Terminal cells keep PTY column widths; fallback glyphs are fitted and clipped to
their cells. Both additional font licenses are included in generated binary legal
sidecars by scripts/third_party_licenses.py.
