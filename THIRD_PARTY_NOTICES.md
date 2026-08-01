# Third-party notices

Bundled font licenses are documented separately in
[`assets/FONT_LICENSES.md`](assets/FONT_LICENSES.md).

## egui_tiles

The dock layout engine in `vendor/egui_tiles` is based on
[egui_tiles](https://github.com/rerun-io/egui_tiles), Copyright (c) 2024
Rerun Technologies AB, under the MIT OR Apache-2.0 license. This distribution
uses the MIT license option; its license text is available at
`vendor/egui_tiles/LICENSE-MIT` in the source repository and
`licenses/egui_tiles-LICENSE-MIT` in release packages.

## Inno Setup Chinese Simplified Translation

The Windows installer uses the `ChineseSimplified.isl` translation maintained
by Zhenghan Yang (Kira), pinned from commit
`6da09d23e14443d4cf8f07b1c5fd821bfe459788` of
[Inno Setup Chinese Simplified Translation](https://github.com/kira-96/Inno-Setup-Chinese-Simplified-Translation).

This translation is licensed under the MIT License. The full license is copied
into release packages as
`licenses/ChineseSimplified-Translation-LICENSE-MIT`.

## egui-notify

The right-top toast notification overlay in `crates/app/src/ui/toast.rs` adapts
interaction and animation ideas from
[egui-notify](https://github.com/ItsEthra/egui-notify), Copyright (c) 2022-2023
ItsEthra, under the MIT License. The implementation is rewritten for this
project's egui 0.35 dependency and notification queue.

```text
MIT License

Copyright (c) 2022-2023 ItsEthra

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
