# Real Logic example schemas

`example-schema.xml`, `example-extension-schema.xml` and `common-types.xml`
are copied verbatim from
[real-logic/simple-binary-encoding](https://github.com/real-logic/simple-binary-encoding)
at tag `1.42.0` (`sbe-samples/src/main/resources/`), licensed under the
Apache License 2.0. Copyright Real Logic Limited.

They are test inputs only: `build.rs` generates codecs from them, and
`tests/car_vectors.rs` checks those codecs against the golden vectors in
`tests/vectors/`.

## Golden vectors

`tests/vectors/car-v0.hex` (207 bytes, `Car` version 0) and `car-v1.hex`
(224 bytes, version 1) come from karyos#646
(<https://github.com/quantstratum/karyos/issues/646>). They were encoded by
the reference SBE tool, `uk.co.real-logic:sbe-all:1.42.0` (SHA-256
`f60becdb567519a2ed3824ffe285f93adb2159b39ede0b234fc8b1f13736236a`), and
derived independently, blind, from the SBE 1.0 specification and the
schema text; both agree byte for byte. The values encoded are those
`tests/car_vectors.rs` sets.
