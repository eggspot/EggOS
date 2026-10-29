# EggFS on-disk format, version 1

EggFS is EggOS's own journaled file system for disks (D131). This document is the format's
specification: enough to read and write an EggFS disk from any implementation. The reference
implementation is the `eggos-eggfs` crate (`eggfs/`); the kernel mounts it through
`kernel/src/vfs/eggfs.rs`.

All integers are little-endian. "CRC-32C" is the Castagnoli CRC (reflected polynomial
`0x82F63B78`, initial value and final XOR `0xFFFFFFFF`; check value of `123456789` is
`0xE3069283`). "CRC-64" is the CRC-64/XZ (reflected polynomial `0xC96C5795D7870F42`,
initial value and final XOR all ones; check value `0x995DC9BBDF1939FA`).

## Blocks and regions

The disk is addressed in **blocks of 4096 bytes**, block 0 first. A volume uses at most
2^32 - 1 - 32768 blocks (about 16 TiB); a larger disk is used up to that. The smallest volume is
256 blocks (1 MiB). In order:

| Region | First block | Blocks |
|--------|-------------|--------|
| Superblock | 0 | 1 |
| Block bitmap | 1 | ceil(blocks / 32768) |
| Inode bitmap | after the block bitmap | ceil(inodes / 32768) |
| Journal | after the inode bitmap | clamp(blocks / 256, 64, 16384) |
| Inode table | after the journal | inodes / 32 |
| Data | after the inode table | the rest |

`inodes` is blocks / 4, at least 32, rounded up to a multiple of 32. Everything before the
data region is the file system's own; directory blocks, pointer blocks and file data live in
the data region.

## Superblock (block 0)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | magic `EGGFS\0\0\0` |
| 8 | 4 | version, 1 |
| 12 | 4 | block size, 4096 |
| 16 | 4 | blocks in the volume |
| 20 | 4 | inodes |
| 24 | 4 × 9 | first block and length of the block bitmap, the inode bitmap, the journal and the inode table, then the first data block |
| 64 | 8 | when the volume was made, seconds since the Unix epoch |
| 4092 | 4 | CRC-32C of bytes 0 to 4091 |

A reader must check every field: the regions in the order above with no gaps, bitmaps large
enough for their bits, the inode table exactly inodes / 32 blocks, a journal of at least 64
blocks and at least 8 data blocks. The superblock is written once, by the format, and never
changes. A disk whose block 0 starts with `EGGFSFMT` is a format that stopped before it ended:
it is blank.

## Bitmaps

Bit *n* of the bitmap (byte *n* / 8 of the region, bit *n* mod 8, least significant first) is
1 when block (or inode) *n* is in use. Blocks 0 to the first data block - 1 are always in use,
as are the bits past the last block or inode, which pad the last bitmap block. Inodes 0 and 1
are always in use: 0 means "none", 1 is the root directory.

## Inodes

Inode *n* is the 128 bytes at offset (*n* mod 32) × 128 of block (inode table start + *n* / 32).

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | kind: 0 free, 1 directory, 2 regular file |
| 2 | 2 | permission bits (`07777`) |
| 4 | 4 | generation, raised each time the inode is freed |
| 8 | 4 | parent directory's inode (the root's is itself) |
| 12 | 4 | subdirectories (directories) |
| 16 | 8 | size in bytes |
| 24 | 8 | last change of the contents, seconds since the Unix epoch |
| 32 | 4 | entries (directories) |
| 36 | 4 | owner's user id |
| 40 | 4 | group id |
| 44 | 4 × 12 | direct block pointers |
| 92 | 4 × 3 | single, double and triple indirect block pointers |
| 124 | 4 | CRC-32C of the inode's number (4 bytes) followed by bytes 0 to 123 |

An inode in use reads back with its checksum right; a free inode's bytes do not matter. A
file or directory has exactly one name, in its parent (no hard links): a directory's link
count is 2 plus its subdirectories, a file's is 1.

## Contents

Block *i* of a file or directory (bytes *i* × 4096 on) is found through the pointers: blocks 0
to 11 through the direct pointers; the next 1024 through the single indirect block, a block of
1024 four-byte pointers; the next 1024² through the double indirect block, whose pointers lead
to such blocks; the next 1024³ through the triple. A pointer of 0 is a hole that reads as
zeros (files only); every other pointer names a block of the data region, used by exactly one
inode once. A file maps no block at or past ceil(size / 4096), and the bytes of its last block
past its size are zeros. The largest file is (12 + 1024 + 1024² + 1024³) blocks.

## Directories

A directory's size is a whole number of blocks, at most 16384, and every block up to its size
is mapped (no holes). Each directory block holds packed **records** from offset 0:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | inode, never 0 |
| 4 | 1 | kind: 1 directory, 2 regular file (the inode's) |
| 5 | 1 | name length, 1 to 255 |
| 6 | length | the name: any bytes but `/` and NUL, not `.` or `..` |

The first record whose inode is 0 (or the end of the block) ends the block's records, and all
bytes after the last record are zeros. Names are unique within a directory. Removing a record
moves the records after it down; a new record goes after the last record of the first block
with room, or into a new block added at the directory's end. `.` and `..` are not stored: the
parent is in the inode. The directory's inode counts its records (entries) and those that are
directories.

## The journal

Every change to the file system's own blocks (bitmaps, inodes, directory and pointer blocks)
goes through the journal, one **transaction** at a time; file data does not. A transaction of
*n* blocks (1 ≤ *n* ≤ min(journal blocks - 2, 1017)) occupies the journal's first *n* + 2 blocks:

- **Descriptor** (journal block 0): magic `EGGFSJDS`, sequence number (8 bytes at 8), *n* (4 at
  16), the *n* target block numbers (4 bytes each from offset 24), and a CRC-32C of bytes 0 to
  4091 at 4092.
- **Images** (journal blocks 1 to *n*): the new contents of each target, in order.
- **Commit** (journal block *n* + 1): magic `EGGFSJCM`, the sequence number (8 at 8), *n* (4 at
  16), the transaction checksum (8 at 20): the CRC-64 of the descriptor block and the *n*
  images in order, and a CRC-32C of bytes 0 to 27 at 28.

The transaction checksum is a CRC-64 rather than a CRC-32C because the blocks it covers carry
CRC-32Cs of their own: a change to an inode that keeps the inode's own checksum right leaves an
enclosing CRC with the same polynomial unchanged, so a torn image could pass. The host crash
tests found this.

**Committing** writes, in this order, with the disk's write cache flushed where marked:

1. the data blocks of the transaction's files (already written by the writes themselves);
2. the descriptor and the images; **flush**;
3. the commit block; **flush**;
4. each image to its target block; **flush**.

A file's data is written **copy on write**: a block the last commit's state uses is never
written in place; new contents go to a free block, which the transaction's pointers then name.
A block freed by the running transaction is not reused until it commits. A block allocated by
the running transaction may be written again in place. So the disk always holds the last
committed state, plus blocks no committed pointer names.

**Mounting** reads the descriptor; if its checksum and the commit block (same sequence, same
*n*, the transaction checksum of what the journal holds) match, it compares each image with
its target and writes those that differ, then flushes. Replaying a transaction already in place
changes nothing, so a journal is never cleared. The next transaction's sequence number is the
descriptor's plus 1 (1 on a blank journal). A torn or uncommitted transaction is ignored: its
targets still hold the state of the commit before, which reached its places before this one
began.

## Formatting

A blank disk (its first 128 KiB zeros, or block 0 starting with `EGGFSFMT`) is formatted by
writing `EGGFSFMT` to block 0, flushing, writing the bitmaps, a zero journal descriptor and the
inode table block holding the root (inode 1, a directory, parent 1, generation 1, 0755),
flushing, then writing the superblock and flushing. EggOS never formats a disk holding
anything else: see "Persistent disk" in docs/build.md for erasing one on purpose.

## Checking

`Volume::check` (also run on the image by the QEMU tests) verifies everything above: every
inode reachable from the root reads back, its kind matches its record and its parent the
directory listing it, every pointer leads into the data region and is used once, nothing is
mapped past a file's end, directories have no holes, malformed records or repeated names and
their counts are right, and the bitmaps mark exactly the blocks and inodes in use.

## Work disks

A **work disk** (D166) is an ordinary EggFS volume whose root directory holds a regular file
named `.eggos-work-disk` (empty, permissions 0444) and a directory named `work`, which EggOS
mounts at `/work`; the disk that holds `/home` has neither (its root holds `home`,
`Applications` and `var`). Nothing inside EggOS can reach a volume's root, so only the host
makes a work disk (`cargo xtask work pack`). EggOS tells the two apart by this marker, never by
the order of the disks: of the EggFS disks it finds, the first with the marker is mounted at
`/work` and the first without it (or the first blank one) at `/home`. The crate's
`make_work_disk`, `is_work_disk` and `work_folder` write and read the marker.
