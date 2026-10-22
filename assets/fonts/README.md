# CJK fallback font

`NotoSansSC-Actx.ttf.zst` is a static weight-400 subset of Noto Sans SC under the SIL Open Font
License 1.1. Copyright and license are in [OFL-NotoSansSC.txt](OFL-NotoSansSC.txt).
It is embedded only with the `gui` feature and decompressed once at window startup.

Source: [Google Fonts / Noto Sans SC](https://github.com/google/fonts/tree/main/ofl/notosanssc),
`NotoSansSC[wght].ttf`. Unicode coverage: U+0020–024F, U+2000–30FF, U+3400–9FFF and U+FF00–FFEE.
This covers common Chinese ideographs, punctuation and the Chinese UI; egui's default fonts
handle other supported glyphs.

To reproduce, install Python fonttools in a temporary virtual environment, instantiate `wght=400`
using `fontTools.varLib.instancer.instantiateVariableFont`, then subset those Unicode ranges
with `fontTools.subset`, preserving name IDs/languages and setting `recalc_timestamp=False`.
Compress the TTF using `zstd -15`. Python tools are not needed to build or run actx.

Source SHA-256: `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`.
Compressed subset SHA-256: `9b8210e7ba1d4b69e0d23508c9fe08514ee3cae07e621c451da879f3f21b82d6`.
