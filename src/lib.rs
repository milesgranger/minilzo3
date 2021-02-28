#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_assignments)]
#![allow(unused_mut)]

extern crate libc;
use std::io;
use std::ffi::c_void;

pub mod raw;

const BLOCK_SIZE: usize = 64000;
const MAX_BLOCK_COMPRESS_SIZE: usize = BLOCK_SIZE + (BLOCK_SIZE / 16) + 64 + 3;


pub fn init() {
    let r = unsafe { raw::lzo_initialize() };
    if !r == 0 {
        panic!("Failed initialize LZO!");
    }
}

pub fn max_compress_len(input_len: usize) -> usize {
    // ref: docs/LZO.FAQ
    input_len + (input_len / 16) + 64 + 3
}

pub fn decompress(input: &[u8], output: &mut [u8]) -> usize {
    init();
    unsafe {
        let mut wrkmem: [u8; 0] = std::mem::uninitialized();
        let mut out_len = 0;
        let r = raw::lzo1x_decompress_safe(
            input.as_ptr(),
            input.len() as u64,
            output.as_mut_ptr(),
            &out_len as *const _ as *mut _,
            wrkmem.as_mut_ptr() as *mut c_void,
        );
        if !r == 0 {
            panic!("Failed to decompress, exit code: {}", r);
        }
        out_len as usize
    }
}

pub fn compress(input: &[u8], output: &mut [u8]) -> usize {
    init();
    unsafe {
        let mut wrkmem: [u8; 64000] = std::mem::uninitialized();

        let mut out_len = 0;
        let v = raw::lzo1x_1_compress(
            input.as_ptr(),
            input.len() as u64,
            output.as_mut_ptr(),
            &out_len as *const _ as *mut _,
            wrkmem.as_mut_ptr() as *mut c_void,
        );
        if !v == 0 {
            panic!("Failed to compress, exit code: {}", v);
        }
        out_len as usize
    }
}

pub struct Encoder<R: io::Read> {
    inner: R,
    src: [u8; BLOCK_SIZE],  // hold extra bytes that didn't fit in destination buffer
    dst: [u8; MAX_BLOCK_COMPRESS_SIZE],
    position: usize,
    remaining_chunk_length: usize
}

impl<R: io::Read> Encoder<R> {
    pub fn new(inner: R) -> Self {
        init();
        Self {
            inner,
            src: [0; BLOCK_SIZE],
            dst: [0; MAX_BLOCK_COMPRESS_SIZE],
            position: 0,
            remaining_chunk_length: 0
        }
    }
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {

        // check if we still had remaining compressed chunk to write out
        if self.position > 0 {
            buf.copy_from_slice(&self.dst[self.position..self.position + self.remaining_chunk_length]);
            let n_bytes = self.remaining_chunk_length;
            self.position = 0;
            self.remaining_chunk_length = 0;
            return Ok(n_bytes)
        }

        // Read from inner into source buffer
        self.src = [0; BLOCK_SIZE];
        let n_bytes = self.inner.read(self.src.as_mut())?;
        if n_bytes == 0 {
            return Ok(0)
        }

        // compress the source buffer
        self.dst = [0; MAX_BLOCK_COMPRESS_SIZE];
        let n_compressed_bytes = compress(&self.src, self.dst.as_mut());

        // write and much as we can into this output buffer
        if n_compressed_bytes <= buf.len() {
            buf[..n_compressed_bytes].copy_from_slice(&self.dst[..n_compressed_bytes]);
            Ok(n_compressed_bytes)  // wrote all
        } else {
            let len = buf.len();
            buf.copy_from_slice(&self.dst[..len]);
            self.position = buf.len();
            self.remaining_chunk_length = n_compressed_bytes - buf.len();
            Ok(buf.len())  // had some left, but filled the buffer.
        }
    }
}

impl<R: io::Read> io::Read for Encoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n_bytes = 0;
        loop {
            let count = self.read_chunk(&mut buf[n_bytes..])?;
            if count == 0 {
                break;
            }
            n_bytes += count;
        }
        Ok(n_bytes)
    }
}

pub struct Decoder<R: io::Read> {
    inner: R,
    src: [u8; MAX_BLOCK_COMPRESS_SIZE],
    dst: [u8; BLOCK_SIZE],
    position: usize,
    remaining_chunk_length: usize
}

impl<R: io::Read> Decoder<R> {
    pub fn new(inner: R) -> Self {
        init();
        Self {
            inner,
            src: [0; MAX_BLOCK_COMPRESS_SIZE],
            dst: [0; BLOCK_SIZE],
            position: 0,
            remaining_chunk_length: 0
        }
    }
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.len() == 0 {
            return Ok(0)
        }
        if self.position > 0 {
            if self.remaining_chunk_length >= buf.len() {
                buf.copy_from_slice(&self.dst[self.position..self.position + buf.len()]);
                self.position += buf.len();
                self.remaining_chunk_length -= buf.len();
                if self.remaining_chunk_length <= 0 {
                    self.remaining_chunk_length = 0;
                    self.position = 0;
                }
                return Ok(buf.len())
            } else {
                buf[..self.remaining_chunk_length].copy_from_slice(&self.dst[self.position..self.position + self.remaining_chunk_length]);
                let n_bytes = self.remaining_chunk_length;
                self.position = 0;
                self.remaining_chunk_length = 0;
                return Ok(n_bytes)
            }

        }

        self.src = [0; MAX_BLOCK_COMPRESS_SIZE];
        let n_bytes = self.inner.read(self.src.as_mut())?;
        if n_bytes == 0 {
            return Ok(0)
        }
        self.dst = [0; BLOCK_SIZE];
        let n_decompressed_bytes = decompress(&self.src[..n_bytes], self.dst.as_mut());
        if n_decompressed_bytes <= buf.len() {
            buf[..n_decompressed_bytes].copy_from_slice(&self.dst[..n_decompressed_bytes]);
            Ok(n_decompressed_bytes)
        } else {
            let len = buf.len();
            buf.copy_from_slice(&self.dst[..len]);
            self.position = buf.len();
            self.remaining_chunk_length = n_decompressed_bytes - buf.len();
            Ok(buf.len())
        }
    }
}

impl<R: io::Read> io::Read for Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n_bytes = 0;
        loop {
            let count = self.read_chunk(&mut buf[n_bytes..])?;
            if count == 0 {
                break;
            }
            n_bytes += count;
        }
        Ok(n_bytes)
    }
}

#[cfg(test)]
mod tests {
    use crate::{compress, decompress, max_compress_len, Encoder, Decoder};

    fn gen_data() -> Vec<u8> {
        (0..10000)
            .map(|_| b"Oh what a beautiful day, oh what a beaitufl morning!!!".to_vec())
            .flat_map(|v| v)
            .collect::<Vec<u8>>()
    }

    #[test]
    fn test_roundtrip() {
        let input = gen_data();

        let mut compressed = vec![0; max_compress_len(input.len())];
        let n_bytes = compress(&input, compressed.as_mut_slice());

        let mut decompressed: Vec<u8> = vec![0; input.len()];
        let n_bytes = decompress(&compressed[..n_bytes], decompressed.as_mut_slice());

        assert_eq!(&decompressed[..n_bytes], input.as_slice());
    }

    #[test]
    fn test_encoder() {
        let data = gen_data()[..64005].to_vec();
        let mut encoder = Encoder::new(data.as_slice());

        let mut compressed = vec![];
        let n = std::io::copy(&mut encoder, &mut compressed).unwrap();
        println!("Output vec size: {}", compressed.len());
        println!("Bytes copied: {}", n);
        println!("compressed: {:?}", &compressed);

        let mut decompressed = vec![0; data.len()];
        let mut decoder = Decoder::new(compressed.as_slice());
        let n = std::io::copy(&mut decoder, &mut decompressed).unwrap();
        println!("Decompressed bytes: {}", decompressed.len());
        println!("copied bytes: {}", n);
        assert_eq!(&decompressed[..data.len()][data.len() - 100..], &data[data.len() - 100..]);
    }

}
