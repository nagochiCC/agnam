# Native RAR test fixtures

These public upstream fixtures contain synthetic/test data, not user archives.

- `rar5-solid.rar`: `unrar` 0.5.8 `data/solid.rar` (RAR5, one compressed file), unchanged. Source: https://github.com/muja/unrar.rs/tree/8fe139781b92d0678afecc568496f5805401a6c4/data
  Licensed MIT OR Apache-2.0; the upstream MIT license is reproduced in `LICENSE-unrar-MIT`.
- `rar5-multiple-solid.rar`: uu-decoded, otherwise unchanged, from libarchive
  `libarchive/test/test_read_format_rar5_multiple_files_solid.rar.uu` at revision
  `3bdd98d298af3ba33fb10c51582f05ddd0584c35`.
  Source: https://github.com/libarchive/libarchive/blob/3bdd98d298af3ba33fb10c51582f05ddd0584c35/libarchive/test/test_read_format_rar5_multiple_files_solid.rar.uu
  SHA256: `0aa2c652c06304dedd93019663c1d52d5e85055d5fdb8045bfcc4345b90b4a0f`.
  The associated upstream test uses the following license:

```text
/*-
 * Copyright (c) 2018 Grzegorz Antoniak
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR(S) ``AS IS'' AND ANY EXPRESS OR
 * IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
 * OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
 * IN NO EVENT SHALL THE AUTHOR(S) BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT
 * NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
 * THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */
```
