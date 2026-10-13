# Bundled fonts and third-party notices

HW Glance embeds these fonts in the window. They are not part of the GPL-3.0-or-later code of the rest of this repository; each keeps its own license.

| Files | Font | License | License file |
|---|---|---|---|
| `AtkinsonHyperlegible-Regular.otf`, `AtkinsonHyperlegible-Bold.otf` | Atkinson Hyperlegible, Copyright 2020 Braille Institute of America, Inc. | SIL Open Font License 1.1 | [`OFL-AtkinsonHyperlegible.txt`](OFL-AtkinsonHyperlegible.txt) |
| `AtkynsonMonoNerdFontMono-Regular.otf`, `-Medium.otf`, `-Bold.otf` | AtkynsonMono Nerd Font Mono: Atkinson Hyperlegible Mono (Braille Institute of America, Inc.) patched with Nerd Fonts icon glyphs | SIL Open Font License 1.1 | [`LICENSE-AtkinsonHyperlegibleMono-NerdFont.txt`](LICENSE-AtkinsonHyperlegibleMono-NerdFont.txt) |

## The embedded text in the AtkynsonMono files

The name tables of the AtkynsonMono files carry license text that speaks of "without derivatives". That is a leftover from the license Braille Institute used for an earlier release of the font. The operative license is the SIL Open Font License 1.1 in the license file above, as published by Nerd Fonts at <https://github.com/ryanoasis/nerd-fonts/blob/master/src/unpatched-fonts/AtkinsonHyperlegibleMono/LICENSE.txt> and packaged as OFL by Arch Linux.

## Icon glyphs in the Nerd Font build

The icon glyphs patched into AtkynsonMono Nerd Font Mono come from the glyph sets that the [Nerd Fonts](https://github.com/ryanoasis/nerd-fonts) project bundles. The ones that matter for licensing:

- Material Design Icons: Apache License 2.0, see <https://pictogrammers.com/docs/general/license/>. A copy of the license text is in [`LICENSE-Apache-2.0-MaterialDesignIcons.txt`](LICENSE-Apache-2.0-MaterialDesignIcons.txt).
- Font Awesome Free: the icons are CC BY 4.0 and the font files SIL OFL 1.1, see <https://fontawesome.com/license/free>.
- Octicons: MIT.
- Devicons and Seti UI: MIT.
- Others, each with its own license: see the Nerd Fonts project, <https://github.com/ryanoasis/nerd-fonts>, and its license page, <https://github.com/ryanoasis/nerd-fonts/blob/master/LICENSE>.

HW Glance draws only a handful of these glyphs (the window controls and small marks), but the whole patched font is embedded as it was released.
