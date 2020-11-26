# gojsonnet-rs
Jsonnet interpreter for Rust using google/go-jsonnet

## Example
```
% cargo run --example jsonnet -- --ext-str=foo=bar --ext-code=hoge=1 -e '{foo: std.extVar("foo"), hoge: std.extVar("hoge") + 1}'
{"foo":"bar","hoge":2}
```
