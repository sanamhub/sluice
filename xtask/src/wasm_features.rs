//! Checks on a built `.wasm` with `wasmparser`: whether it uses SIMD, and which target features the
//! toolchain says it was compiled with.

use anyhow::{Result, bail};
use wasmparser::{BinaryReader, Parser, Payload, Validator, WasmFeatures};

/// What the validator accepts when SIMD is switched off and everything else is on. A module that
/// fails only under this set uses SIMD, and nothing else.
fn without_simd() -> WasmFeatures {
    WasmFeatures::all() - WasmFeatures::SIMD - WasmFeatures::RELAXED_SIMD
}

fn validate(wasm: &[u8], features: WasmFeatures) -> Result<(), wasmparser::BinaryReaderError> {
    Validator::new_with_features(features)
        .validate_all(wasm)
        .map(|_| ())
}

/// The scalar build must validate with SIMD off, and the simd build must not. A `+simd128` that
/// never reached the simd build would otherwise ship a second scalar build under its name, and
/// the page would report SIMD numbers that are not.
pub fn check_simd(name: &str, wasm: &[u8], expect_simd: bool) -> Result<()> {
    if let Err(error) = validate(wasm, WasmFeatures::all()) {
        bail!("the {name} build is not a valid module: {error}");
    }
    let off = validate(wasm, without_simd());
    match (expect_simd, off) {
        (false, Err(error)) => bail!("the {name} build uses SIMD, and it must not: {error}"),
        (true, Ok(())) => {
            bail!("the {name} build uses no SIMD instruction, so +simd128 was not applied")
        }
        _ => Ok(()),
    }
}

/// The `+feature` entries of the `target_features` section. Rust writes it from the features the
/// module was compiled with, and `wasm-bindgen` keeps it, so a load failure in an old browser can be
/// traced to a name in this list.
pub fn target_features(wasm: &[u8]) -> Result<Vec<String>> {
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CustomSection(section) = payload? else {
            continue;
        };
        if section.name() != "target_features" {
            continue;
        }
        let mut reader = BinaryReader::new(section.data(), section.data_offset());
        let mut features = Vec::new();
        for _ in 0..reader.read_var_u32()? {
            let prefix = reader.read_u8()?;
            let name = reader.read_string()?;
            if prefix == b'+' {
                features.push(name.to_string());
            }
        }
        return Ok(features);
    }
    bail!("the module has no target_features section")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &[u8] = b"\0asm\x01\0\0\0";

    // One function that returns a v128, the module the page feeds to `WebAssembly.validate`.
    const SIMD: &[u8] = &[
        0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0,
        253, 15, 253, 98, 11,
    ];

    fn with_target_features(entries: &[(u8, &str)]) -> Vec<u8> {
        let mut section = vec![15];
        section.extend_from_slice(b"target_features");
        section.push(entries.len() as u8);
        for (prefix, name) in entries {
            section.push(*prefix);
            section.push(name.len() as u8);
            section.extend_from_slice(name.as_bytes());
        }
        let mut module = EMPTY.to_vec();
        module.push(0);
        module.push(section.len() as u8);
        module.extend(section);
        module
    }

    #[test]
    fn a_module_without_simd_passes_as_scalar_and_fails_as_simd() {
        assert!(check_simd("scalar", EMPTY, false).is_ok());
        let error = check_simd("simd", EMPTY, true).unwrap_err().to_string();
        assert!(error.contains("+simd128 was not applied"), "{error}");
    }

    #[test]
    fn a_module_with_simd_passes_as_simd_and_fails_as_scalar() {
        assert!(check_simd("simd", SIMD, true).is_ok());
        let error = check_simd("scalar", SIMD, false).unwrap_err().to_string();
        assert!(error.contains("uses SIMD"), "{error}");
    }

    #[test]
    fn a_broken_module_is_reported_as_invalid_and_not_as_a_simd_mismatch() {
        let error = check_simd("scalar", b"\0asm\x01\0\0\0\x01", false).unwrap_err();
        assert!(error.to_string().contains("not a valid module"), "{error}");
    }

    #[test]
    fn target_features_keeps_the_plus_entries_only() {
        let module =
            with_target_features(&[(b'+', "simd128"), (b'-', "atomics"), (b'+', "sign-ext")]);
        assert_eq!(target_features(&module).unwrap(), ["simd128", "sign-ext"]);
    }

    #[test]
    fn a_module_without_the_section_is_an_error() {
        assert!(target_features(EMPTY).is_err());
    }
}
