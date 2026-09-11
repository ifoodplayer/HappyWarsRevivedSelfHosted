use std::path::Path;

use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::io::{Read, Write};
use tracing::warn;

pub const PAK_MAGIC: &[u8; 4] = b"KPK|";
pub const PAK_NAME_STRIDE: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    Zres,
    Bres,
    Plaintext,
}

#[derive(Debug, Clone)]
pub struct PakEntry {
    pub name: String,
    pub kind: EntryKind,
    pub data: Vec<u8>, // decompressed
}

// ─── ZRES ────────────────────────────────────────────────────────────────────

pub fn zres_compress_le(raw: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(6));
    enc.write_all(raw).unwrap();
    let compressed = enc.finish().unwrap();
    let mut out = Vec::with_capacity(8 + compressed.len());
    out.extend_from_slice(b"ZRES");
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

/// Builds a ZRES block for Xbox 360: standard zlib with the adler32 checksum stored big-endian.
/// Layout: "ZRES" | decomp_size (LE u32) | 0x78 0x9C | deflate body | adler32 (BE u32)
pub fn zres_compress_be(raw: &[u8]) -> Vec<u8> {
    // Standard zlib produces: 0x78 0x9C | deflate body | adler32 (LE) — strip header and LE checksum
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(6));
    enc.write_all(raw).unwrap();
    let zlib_data = enc.finish().unwrap();

    let deflate_body = &zlib_data[2..zlib_data.len() - 4];

    let adler = adler32::adler32(raw).unwrap_or(1);

    // Reassemble with BE adler32
    let zlib_be: Vec<u8> = std::iter::once(0x78u8)
        .chain(std::iter::once(0x9Cu8))
        .chain(deflate_body.iter().copied())
        .chain(adler.to_be_bytes())
        .collect();

    let mut out = Vec::with_capacity(8 + zlib_be.len());
    out.extend_from_slice(b"ZRES");
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(&zlib_be);
    out
}

pub fn zres_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 8 {
        return Err("ZRES too short".into());
    }
    let compressed = &data[8..];
    let mut dec = ZlibDecoder::new(compressed);
    let mut out = Vec::new();
    if dec.read_to_end(&mut out).is_ok() {
        return Ok(out);
    }
    // Retry after stripping trailing null padding
    let trimmed = {
        let mut t = compressed;
        while t.last() == Some(&0) {
            t = &t[..t.len() - 1];
        }
        t
    };
    let mut dec = ZlibDecoder::new(trimmed);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

// ─── PAK LE parse ────────────────────────────────────────────────────────────

/// Parses a little-endian KPK| PAK file into a list of entries with decompressed payloads.
pub fn pak_parse_le(pak: &[u8]) -> Result<Vec<PakEntry>, String> {
    if pak.len() < 16 {
        return Err("PAK too short".into());
    }
    if &pak[..4] != PAK_MAGIC {
        return Err(format!("Bad magic: {:?}", &pak[..4]));
    }

    let num = u32::from_le_bytes(pak[4..8].try_into().unwrap()) as usize;
    if num == 0 {
        return Ok(Vec::new());
    }
    let first_offset = u32::from_le_bytes(pak[12..16].try_into().unwrap()) as usize;

    // Directory starts at byte 16 and holds (num-1) offsets; entry[0]'s offset is in the header
    let dir_off = 16usize;
    let mut offsets = vec![first_offset];
    for i in 0..num.saturating_sub(1) {
        let start = dir_off + i * 4;
        if start + 4 > pak.len() {
            return Err("PAK directory truncated".into());
        }
        offsets.push(u32::from_le_bytes(pak[start..start + 4].try_into().unwrap()) as usize);
    }

    // Block sizes table follows the directory
    let sizes_off = dir_off + num.saturating_sub(1) * 4;
    if sizes_off + num * 4 > pak.len() {
        return Err("PAK sizes table truncated".into());
    }
    let mut sizes = Vec::with_capacity(num);
    for i in 0..num {
        let start = sizes_off + i * 4;
        sizes.push(u32::from_le_bytes(pak[start..start + 4].try_into().unwrap()) as usize);
    }

    // Name table: u32 stride followed by (num * stride) bytes of null-padded names
    let name_table_off = sizes_off + num * 4;
    if name_table_off + 4 > pak.len() {
        return Err("PAK name table offset overflow".into());
    }
    let stride = u32::from_le_bytes(pak[name_table_off..name_table_off + 4].try_into().unwrap()) as usize;
    if stride == 0 || stride > 4096 {
        return Err(format!("Invalid PAK name stride: {stride}"));
    }
    let names_start = name_table_off + 4;
    let mut names = Vec::with_capacity(num);
    for i in 0..num {
        let start = names_start + i * stride;
        let end = start + stride;
        let raw = if end <= pak.len() {
            &pak[start..end]
        } else if start < pak.len() {
            &pak[start..]
        } else {
            &[]
        };
        let s = raw.split(|&b| b == 0).next().unwrap_or(&[]);
        names.push(String::from_utf8_lossy(s).into_owned());
    }

    let mut entries = Vec::new();
    for i in 0..num {
        let pos = offsets[i];
        let bsize = sizes[i];
        if bsize < 4 {
            continue;
        }
        let block_end = pos.checked_add(bsize)
            .ok_or_else(|| format!("PAK entry {} offset overflow", names[i]))?;
        if block_end > pak.len() {
            return Err(format!("PAK entry {} truncated", names[i]));
        }
        let raw_block = &pak[pos..block_end];
        let tag = &raw_block[..4];

        if tag == b"ZRES" {
            match zres_decompress(raw_block) {
                Ok(data) => entries.push(PakEntry {
                    name: names[i].clone(),
                    kind: EntryKind::Zres,
                    data,
                }),
                Err(e) => warn!("[pak] zres decomp error entry {}: {e}", names[i]),
            }
        } else if tag == b"BRES" {
            entries.push(PakEntry {
                name: names[i].clone(),
                kind: EntryKind::Bres,
                data: raw_block.to_vec(),
            });
        } else {
            let kind = if std::str::from_utf8(raw_block).is_ok() {
                EntryKind::Plaintext
            } else {
                EntryKind::Bres
            };
            entries.push(PakEntry {
                name: names[i].clone(),
                kind,
                data: raw_block.to_vec(),
            });
        }
    }

    Ok(entries)
}

// ─── PAK LE build ────────────────────────────────────────────────────────────

/// Builds a little-endian KPK| PAK from the given entries.
pub fn pak_build_le(entries: &[PakEntry]) -> Vec<u8> {
    let num = entries.len();
    let sentinel_count = 0usize;
    let total = num + sentinel_count;
    let name_stride = PAK_NAME_STRIDE;
    let data_align = 256usize;

    let blocks: Vec<Vec<u8>> = entries
        .iter()
        .map(|e| match e.kind {
            EntryKind::Zres => zres_compress_le(&e.data),
            _ => e.data.clone(),
        })
        .collect();

    let header_size = 16;
    let dir_size = total.saturating_sub(1) * 4;
    let sizes_size = total * 4;
    let names_size = 4 + total * name_stride;
    let mut data_start = header_size + dir_size + sizes_size + names_size;
    if data_align > 1 && data_start % data_align != 0 {
        data_start = (data_start / data_align + 1) * data_align;
    }

    let mut offsets = Vec::with_capacity(num);
    let mut pad_after = Vec::with_capacity(num);
    let mut cur = data_start;
    for (i, blk) in blocks.iter().enumerate() {
        offsets.push(cur);
        cur += blk.len();
        let is_last = (i + 1 == blocks.len()) && sentinel_count == 0;
        if data_align > 1 && cur % data_align != 0 && !is_last {
            let aligned = (cur / data_align + 1) * data_align;
            pad_after.push(aligned - cur);
            cur = aligned;
        } else {
            pad_after.push(0);
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"KPK|");
    buf.extend_from_slice(&(total as u32).to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
    buf.extend_from_slice(&(offsets[0] as u32).to_le_bytes());
    for &o in &offsets[1..] {
        buf.extend_from_slice(&(o as u32).to_le_bytes());
    }
    for blk in &blocks {
        buf.extend_from_slice(&(blk.len() as u32).to_le_bytes());
    }
    buf.extend_from_slice(&(name_stride as u32).to_le_bytes());
    for e in entries {
        let name_bytes = e.name.as_bytes();
        let mut n = [0u8; PAK_NAME_STRIDE];
        let len = name_bytes.len().min(PAK_NAME_STRIDE);
        n[..len].copy_from_slice(&name_bytes[..len]);
        buf.extend_from_slice(&n);
    }
    if buf.len() < data_start {
        buf.resize(data_start, 0);
    }
    for (blk, pad) in blocks.iter().zip(pad_after.iter()) {
        buf.extend_from_slice(blk);
        buf.extend(std::iter::repeat(0u8).take(*pad));
    }
    buf
}

// ─── PAK BE build (Xbox 360) ─────────────────────────────────────────────────

/// Builds a big-endian KPK| PAK for Xbox 360. Includes a sentinel entry at the end.
pub fn pak_build_be(entries: &[PakEntry]) -> Vec<u8> {
    let num = entries.len();
    let sentinel_count = 1usize;
    let total = num + sentinel_count;
    let name_stride = PAK_NAME_STRIDE;
    let data_align = 128usize;

    let blocks: Vec<Vec<u8>> = entries
        .iter()
        .map(|e| match e.kind {
            EntryKind::Zres => zres_compress_be(&e.data),
            _ => e.data.clone(),
        })
        .collect();

    let header_size = 16;
    let dir_size = (total - 1) * 4;
    let sizes_size = total * 4;
    let names_size = 4 + total * name_stride;
    let mut data_start = header_size + dir_size + sizes_size + names_size;
    if data_align > 1 && data_start % data_align != 0 {
        data_start = (data_start / data_align + 1) * data_align;
    }

    let mut offsets = Vec::with_capacity(num);
    let mut pad_after = Vec::with_capacity(num);
    let mut cur = data_start;
    for (i, blk) in blocks.iter().enumerate() {
        offsets.push(cur);
        cur += blk.len();
        // sentinel_count is always 1 here, so real entries always get padding
        let is_last = i + 1 == blocks.len() && sentinel_count == 0;
        if data_align > 1 && cur % data_align != 0 && !is_last {
            let aligned = (cur / data_align + 1) * data_align;
            pad_after.push(aligned - cur);
            cur = aligned;
        } else {
            pad_after.push(0);
        }
    }
    let sentinel_offset = cur;

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"KPK|");
    buf.extend_from_slice(&(total as u32).to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes());
    if !offsets.is_empty() {
        buf.extend_from_slice(&(offsets[0] as u32).to_be_bytes());
        for &o in &offsets[1..] {
            buf.extend_from_slice(&(o as u32).to_be_bytes());
        }
    }
    buf.extend_from_slice(&(sentinel_offset as u32).to_be_bytes());
    for blk in &blocks {
        buf.extend_from_slice(&(blk.len() as u32).to_be_bytes());
    }
    buf.extend_from_slice(&2u32.to_be_bytes()); // sentinel size
    buf.extend_from_slice(&(name_stride as u32).to_be_bytes());
    for e in entries {
        let name_bytes = e.name.as_bytes();
        let mut n = [0u8; PAK_NAME_STRIDE];
        let len = name_bytes.len().min(PAK_NAME_STRIDE);
        n[..len].copy_from_slice(&name_bytes[..len]);
        buf.extend_from_slice(&n);
    }
    buf.extend(std::iter::repeat(0u8).take(name_stride)); // sentinel name
    if buf.len() < data_start {
        buf.resize(data_start, 0);
    }
    for (blk, pad) in blocks.iter().zip(pad_after.iter()) {
        buf.extend_from_slice(blk);
        buf.extend(std::iter::repeat(0u8).take(*pad));
    }
    buf.extend_from_slice(&[0u8; 2]); // sentinel data
    buf
}

// ─── LE → BE conversion ───────────────────────────────────────────────────────

/// Converts a little-endian KPK| PAK (or standalone ZRES) to Xbox 360 big-endian format.
/// BRES entries are converted to SERB and re-wrapped in a BE ZRES block.
/// An outer ZRES wrapper, if present, is stripped before conversion and re-applied afterwards.
pub fn pak_convert_to_360(raw: &[u8]) -> Result<Vec<u8>, String> {
    // If ZRES-wrapped but inner content is not a PAK, treat as standalone ZSON
    if raw.starts_with(b"ZRES") {
        let inner = zres_decompress(raw).unwrap_or_default();
        if !inner.starts_with(b"KPK|") {
            // Standalone ZRES — decompress and recompress with BE adler32
            let payload = zres_decompress(raw)?;
            return Ok(zres_compress_be(&payload));
        }
    }

    // Unwrap outer ZRES if present
    let (pak_bytes, outer_zres) = if raw.starts_with(b"ZRES") {
        (zres_decompress(raw)?, true)
    } else {
        (raw.to_vec(), false)
    };

    let entries = pak_parse_le(&pak_bytes)?;
    if entries.is_empty() {
        return Err("File has no entries; skipping conversion".to_string());
    }

    let mut converted: Vec<PakEntry> = Vec::with_capacity(entries.len());

    for e in entries {
        // For BRES entries the decompressed data field holds the raw block bytes
        let raw_block = &e.data;

        if raw_block.starts_with(b"BRES") {
            // bare BRES LE → ZRES(SERB BE)
            let be_data = bres_le_to_be(raw_block);
            converted.push(PakEntry { name: e.name, kind: EntryKind::Zres, data: be_data });
        } else if e.kind == EntryKind::Zres && e.data.starts_with(b"BRES") {
            // ZRES wrapping BRES LE → ZRES(SERB BE)
            let be_data = bres_le_to_be(&e.data);
            converted.push(PakEntry { name: e.name, kind: EntryKind::Zres, data: be_data });
        } else if e.kind == EntryKind::Bres {
            // Try converting as .lng; keep as-is if it isn't one
            match convert_lng_le_to_be(raw_block) {
                Ok(be_data) => converted.push(PakEntry { name: e.name, kind: EntryKind::Bres, data: be_data }),
                Err(_) => converted.push(e),
            }
        } else {
            converted.push(e);
        }
    }

    let mut be_bytes = pak_build_be(&converted);

    if outer_zres {
        be_bytes = zres_compress_be(&be_bytes);
    }

    Ok(be_bytes)
}

// ─── BRES LE → BE (chunk walker) ─────────────────────────────────────────────

/// Converts a little-endian BRES to big-endian SERB in-place.
/// All block tags are byte-reversed, size fields are byteswapped, and
/// PI u32 payloads, .lng, and .idlst FB payloads are converted accordingly.
pub fn bres_le_to_be(src: &[u8]) -> Vec<u8> {
    let mut data = src.to_vec();
    let data_len = data.len();
    let mut current_file_name = String::new();
    bres_walk_le_to_be(&mut data, 0, data_len, &mut current_file_name);

    // Write version = 1 and BOM = 0x11223344 at the fixed offsets expected by the 360 runtime
    if data.len() >= 0x18 {
        data[0x10..0x14].copy_from_slice(&1u32.to_be_bytes());
        data[0x14..0x18].copy_from_slice(&0x11223344u32.to_be_bytes());
    }
    data
}

fn bres_walk_le_to_be(data: &mut Vec<u8>, start: usize, end: usize, file_name: &mut String) {
    let mut pos = start;
    while pos + 8 <= end && pos + 8 <= data.len() {
        let tag_le: [u8; 4] = data[pos..pos + 4].try_into().unwrap();
        let sf_le = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        let is_container = (sf_le & 0x80000000) != 0;
        let payload_size = (sf_le & 0x7FFFFFFF) as usize;

        // 1. Reverse tag bytes in-place
        data[pos..pos + 4].reverse();
        // 2. Byteswap size field LE→BE
        data[pos + 4..pos + 8].copy_from_slice(&sf_le.to_be_bytes());

        let p_start = pos + 8;
        let p_end = p_start + payload_size;
        // Align to 4 bytes for the next block (pad4 rule)

        if is_container {
            if &tag_le == b"FILE" {
                *file_name = String::new();
            }
            let p_end_clamped = p_end.min(data.len());
            bres_walk_le_to_be(data, p_start, p_end_clamped, file_name);
            // Advance past the payload — padding is handled at the parent level
            pos = p_end;
        } else {
            // PI: key\0 + u32 LE — byteswap the u32
            if &tag_le == b"PI  " {
                let p_end_c = p_end.min(data.len());
                if let Some(null_off) = data[p_start..p_end_c].iter().position(|&b| b == 0) {
                    let u32_off = p_start + null_off + 1;
                    if u32_off + 4 <= p_end_c {
                        let v = u32::from_le_bytes(data[u32_off..u32_off + 4].try_into().unwrap());
                        data[u32_off..u32_off + 4].copy_from_slice(&v.to_be_bytes());
                    }
                }
            }

            // FB: per-name payload conversion for .lng / .idlst
            if &tag_le == b"FB  " {
                let ext = Path::new(file_name.as_str())
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                let p_end_c = p_end.min(data.len());
                if ext == "lng" {
                    let orig = data[p_start..p_end_c].to_vec();
                    if let Ok(converted) = convert_lng_le_to_be(&orig) {
                        if converted.len() == orig.len() {
                            data[p_start..p_end_c].copy_from_slice(&converted);
                        } else {
                            warn!("[bres] lng size changed for {file_name}: {} -> {}, skipping", orig.len(), converted.len());
                        }
                    }
                } else if ext == "idlst" {
                    let orig = data[p_start..p_end_c].to_vec();
                    if let Ok(converted) = convert_idlst_le_to_be(&orig) {
                        if converted.len() == orig.len() {
                            data[p_start..p_end_c].copy_from_slice(&converted);
                        } else {
                            warn!("[bres] idlst size changed for {file_name}: {} -> {}, skipping", orig.len(), converted.len());
                        }
                    }
                }
            }

            // PS: scan key/value pairs for 'name' and 'target'.
            if &tag_le == b"PS  " {
                let p_end_c = p_end.min(data.len());
                let chunk = data[p_start..p_end_c].to_vec();
                // Split on null without filtering empties to preserve key/value pair alignment
                let parts: Vec<&[u8]> = chunk.split(|&b| b == 0).collect();
                let mut i = 0usize;
                while i + 1 < parts.len() {
                    let key = parts[i];
                    let val = parts[i + 1];
                    if key == b"name" {
                        *file_name = String::from_utf8_lossy(val).into_owned();
                        break;
                    }
                    if key == b"target" && val == b"Win32" {
                        let old = b"Win32\x00";
                        let new = b"Xbox360\x00";
                        // Replace in-place; only succeeds if there is enough room (Xbox360\0 is 2 bytes longer)
                        let search = &data[p_start..p_end_c];
                        if let Some(off) = search.windows(old.len()).position(|w| w == old) {
                            let idx = p_start + off;
                            if idx + new.len() <= p_end_c {
                                data[idx..idx + new.len()].copy_from_slice(new);
                            }
                        }
                        break;
                    }
                    i += 2;
                }
            }

            pos = p_end;
        }
    }
}

// ─── Payload converters ───────────────────────────────────────────────────────

fn swap32_in(data: &mut [u8], off: usize) {
    if off + 4 > data.len() {
        return;
    }
    let v = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
    data[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

/// Byteswaps the header fields and entry table of a .lng payload from LE to BE.
/// Also swaps the UTF-16LE value strings to UTF-16BE.
fn convert_lng_le_to_be(payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() < 0x60 {
        return Err(format!("LNG too short: {} bytes", payload.len()));
    }
    let mut data = payload.to_vec();

    swap32_in(&mut data, 0x08);
    swap32_in(&mut data, 0x0C);

    let count = u32::from_le_bytes(data[0x40..0x44].try_into().unwrap()) as usize;
    let doff = u32::from_le_bytes(data[0x48..0x4C].try_into().unwrap()) as usize;

    if count > 0x10_000 {
        return Err(format!("count={count} looks invalid"));
    }
    let entries_end = 0x58 + count * 8;
    if entries_end > data.len() {
        return Err("entry table overflows buffer".into());
    }

    let val_base = 0x20 + doff;
    // Walk every value string (null-null terminated UTF-16) to find where the value region ends
    let mut max_end = val_base;
    for i in 0..count {
        let pos = 0x58 + i * 8;
        let voff = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let mut p = (val_base + voff) & !1;
        let limit = data.len().saturating_sub(1);
        while p < limit {
            if data[p] == 0 && data[p + 1] == 0 {
                p += 2;
                break;
            }
            p += 2;
        }
        if p > max_end {
            max_end = p;
        }
    }

    for x in (0x40..0x50).step_by(4) {
        swap32_in(&mut data, x);
    }
    let mut pos = 0x58;
    for _ in 0..count {
        swap32_in(&mut data, pos);
        swap32_in(&mut data, pos + 4);
        pos += 8;
    }
    for i in (val_base..max_end).step_by(2) {
        if i + 1 < data.len() {
            data.swap(i, i + 1);
        }
    }
    Ok(data)
}

/// Byteswaps the header and entry table of an .idlst payload from LE to BE.
fn convert_idlst_le_to_be(payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() < 0x20 {
        return Err(format!("IDLST too short: {} bytes", payload.len()));
    }
    let mut data = payload.to_vec();
    let count = u32::from_le_bytes(data[0x08..0x0C].try_into().unwrap()) as usize;
    for x in (0x08..0x20).step_by(4) {
        swap32_in(&mut data, x);
    }
    let mut pos = 0x20;
    for _ in 0..count {
        if pos + 8 > data.len() {
            break;
        }
        swap32_in(&mut data, pos);
        swap32_in(&mut data, pos + 4);
        pos += 8;
    }
    Ok(data)
}