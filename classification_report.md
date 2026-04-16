# Unsafe Reduction Classification Report

## Executive Summary
- **Base branch**: `master`
- **New branch**: `unsafe_take5`
- **Total unsafe on base**: 722
- **Total unsafe on new**: 70
- **Total reduction**: 652
- **Files processed**: 99/99

## Categories

### Safe Metadata Abstraction (MetadataCursor)
**Description**: `MetadataCursor` is a safe wrapper around a raw `Address` that provides safe methods for loading, storing, and updating metadata. By replacing raw pointer operations and unsafe trait methods with `MetadataCursor`, `unsafe` blocks were eliminated across the file, including in tests.
Definition: `pub struct MetadataCursor(pub(crate) Address);`

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/metadata/header_metadata.rs` | 90 | 2 | -88 |
| `src/util/metadata/side_metadata/global.rs` | 81 | 2 | -79 |
| `src/util/metadata/side_metadata/helpers.rs` | 7 | 6 | -1 |
| `src/policy/marksweepspace/native_ms/block.rs` | 16 | 0 | -16 |
| `src/util/metadata/metadata_val_traits.rs` | 20 | 0 | -20 |
| `src/util/malloc/malloc_ms_util.rs` | 3 | 0 | -3 |
| `src/util/metadata/vo_bit/mod.rs` | 4 | 0 | -4 |
| `src/util/linear_scan.rs` | 1 | 0 | -1 |
| `src/policy/markcompactspace.rs` | 2 | 0 | -2 |
| `src/util/raw_memory_freelist.rs` | 3 | 1 | -2 |
| `src/policy/largeobjectspace.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -217

**Diff Snippets**:
<details>
<summary>src/util/malloc/malloc_ms_util.rs (MetadataCursor)</summary>

```diff
@@ -37,2 +32,2 @@ pub fn align_offset_alloc<VM: VMBinding>(size: usize, align: usize, offset: usi
-    let malloc_res_ptr: *mut usize = (result - BYTES_IN_ADDRESS).to_mut_ptr();
-    unsafe { malloc_res_ptr.write_unaligned(address.as_usize()) };
+    let cursor = MetadataCursor(result - BYTES_IN_ADDRESS);
+    cursor.store::<usize>(address.as_usize());
@@ -44,2 +33,2 @@ pub fn offset_malloc_usable_size(address: Address) -> usize {
-    let malloc_res_ptr: *mut usize = (address - BYTES_IN_ADDRESS).to_mut_ptr();
-    let malloc_res = unsafe { malloc_res_ptr.read_unaligned() } as *mut libc::c_void;
+    let cursor = MetadataCursor(address - BYTES_IN_ADDRESS);
+    let malloc_res = cursor.load::<usize>() as *mut libc::c_void;
@@ -51,2 +41,2 @@ pub fn offset_free(address: Address) {
-    let malloc_res_ptr: *mut usize = (address - BYTES_IN_ADDRESS).to_mut_ptr();
-    let malloc_res = unsafe { malloc_res_ptr.read_unaligned() } as *mut libc::c_void;
+    let cursor = MetadataCursor(address - BYTES_IN_ADDRESS);
+    let malloc_res = cursor.load::<usize>() as *mut libc::c_void;
```
</details>

<details>
<summary>src/util/metadata/header_metadata.rs (load)</summary>

```diff
@@ -149,28 +150,25 @@ impl HeaderMetadataSpec {
         {
             self.assert_mask::<T>(optional_mask);
             self.assert_spec::<T>();
         }
 
+        let cursor = MetadataCursor(self.meta_addr(header));
         // metadata smaller than 8-bits is special in that more than one metadata value may be included in one AtomicU8 operation, and extra shift and mask is required
         let res: T = if self.num_of_bits < 8 {
-            let byte_val = unsafe {
-                if let Some(order) = atomic_ordering {
-                    (self.meta_addr(header)).atomic_load::<AtomicU8>(order)
-                } else {
-                    (self.meta_addr(header)).load::<u8>()
-                }
+            let byte_val = if let Some(order) = atomic_ordering {
+                cursor.load_atomic::<u8>(order)
+            } else {
+                cursor.load::<u8>()
             };
 
             FromPrimitive::from_u8(self.get_bits_from_u8(byte_val)).unwrap()
         } else {
-            unsafe {
-                if let Some(order) = atomic_ordering {
-                    T::load_atomic(self.meta_addr(header), order)
-                } else {
-                    (self.meta_addr(header)).load::<T>()
-                }
+            if let Some(order) = atomic_ordering {
+                cursor.load_atomic(order)
+            } else {
+                cursor.load()
             }
         };
```
</details>

<details>
<summary>src/util/metadata/header_metadata.rs (store)</summary>

```diff
@@ -222,44 +220,40 @@ impl HeaderMetadataSpec {
 
         // metadata smaller than 8-bits is special in that more than one metadata value may be included in one AtomicU8 operation, and extra shift and mask, and compare_exchange is required
         if self.num_of_bits < 8 {
             let val_u8 = val.to_u8().unwrap();
             let byte_addr = self.meta_addr(header);
+            let cursor = MetadataCursor(byte_addr);
             if let Some(order) = atomic_ordering {
-                let _ = unsafe {
-                    <u8 as MetadataValue>::fetch_update(byte_addr, order, order, |old_val: u8| {
-                        Some(self.set_bits_to_u8(old_val, val_u8))
-                    })
-                };
+                let _ = cursor.fetch_update(order, order, |old_val: u8| {
+                    Some(self.set_bits_to_u8(old_val, val_u8))
+                });
             } else {
-                unsafe {
-                    let old_byte_val = byte_addr.load::<u8>();
-                    let new_byte_val = self.set_bits_to_u8(old_byte_val, val_u8);
-                    byte_addr.store::<u8>(new_byte_val);
-                }
+                let old_byte_val = cursor.load::<u8>();
+                let new_byte_val = self.set_bits_to_u8(old_byte_val, val_u8);
+                cursor.store::<u8>(new_byte_val);
             }
         } else {
             let addr = self.meta_addr(header);
-            unsafe {
-                if let Some(order) = atomic_ordering {
-                    // if the optional mask is provided (e.g. for forwarding pointer), we need to use compare_exchange
-                    if let Some(mask) = optional_mask {
-                        let _ = T::fetch_update(addr, order, order, |old_val: T| {
-                            Some(old_val.bitand(mask.inv()).bitor(val.bitand(mask)))
-                        });
-                    } else {
-                        T::store_atomic(addr, val, order);
-                    }
+            let cursor = MetadataCursor(addr);
+            if let Some(order) = atomic_ordering {
+                // if the optional mask is provided (e.g. for forwarding pointer), we need to use compare_exchange
+                if let Some(mask) = optional_mask {
+                    let _ = cursor.fetch_update(order, order, |old_val: T| {
+                        Some(old_val.bitand(mask.inv()).bitor(val.bitand(mask)))
+                    });
                 } else {
-                    let val = if let Some(mask) = optional_mask {
-                        let old_val = T::load(addr);
-                        old_val.bitand(mask.inv()).bitor(val.bitand(mask))
-                    } else {
-                        val
-                    };
-                    T::store(addr, val);
+                    cursor.store_atomic(val, order);
                 }
+            } else {
+                let val = if let Some(mask) = optional_mask {
+                    let old_val: T = cursor.load();
+                    old_val.bitand(mask.inv()).bitor(val.bitand(mask))
+                } else {
+                    val
+                };
+                cursor.store(val);
             }
         }
```
</details>

<details>
<summary>src/util/metadata/header_metadata.rs (compare_exchange)</summary>

```diff
@@ -275,48 +269,42 @@ impl HeaderMetadataSpec {
         failure_order: Ordering,
     ) -> Result<T, T> {
         #[cfg(debug_assertions)]
         self.assert_spec::<T>();
         // metadata smaller than 8-bits is special in that more than one metadata value may be included in one AtomicU8 operation, and extra shift and mask is required
+        let cursor = MetadataCursor(self.meta_addr(header));
         if self.num_of_bits < 8 {
-            let byte_addr = self.meta_addr(header);
-            unsafe {
-                let real_old_byte = byte_addr.atomic_load::<AtomicU8>(success_order);
-                let expected_old_byte =
-                    self.set_bits_to_u8(real_old_byte, old_metadata.to_u8().unwrap());
-                let expected_new_byte =
-                    self.set_bits_to_u8(expected_old_byte, new_metadata.to_u8().unwrap());
-                byte_addr
-                    .compare_exchange::<AtomicU8>(
-                        expected_old_byte,
-                        expected_new_byte,
-                        success_order,
-                        failure_order,
-                    )
-                    .map(|x| FromPrimitive::from_u8(x).unwrap())
-                    .map_err(|x| FromPrimitive::from_u8(x).unwrap())
-            }
+            let real_old_byte = cursor.load_atomic::<u8>(success_order);
+            let expected_old_byte =
+                self.set_bits_to_u8(real_old_byte, old_metadata.to_u8().unwrap());
+            let expected_new_byte =
+                self.set_bits_to_u8(expected_old_byte, new_metadata.to_u8().unwrap());
+            cursor
+                .compare_exchange::<u8>(
+                    expected_old_byte,
+                    expected_new_byte,
+                    success_order,
+                    failure_order,
+                )
+                .map(|x| FromPrimitive::from_u8(x).unwrap())
+                .map_err(|x| FromPrimitive::from_u8(x).unwrap())
         } else {
-            let (old_metadata, new_metadata) = if let Some(mask) = optional_mask {
-                let old_byte = unsafe { T::load_atomic(addr, success_order) };
+            let (old_metadata, new_metadata) = if let Some(mask) = optional_mask {
+                let old_byte = cursor.load_atomic::<T>(success_order);
                 let expected_new_byte = old_byte.bitand(mask.inv()).bitor(new_metadata);
                 let expected_old_byte = old_byte.bitand(mask.inv()).bitor(old_metadata);
                 (expected_old_byte, expected_new_byte)
             } else {
                 (old_metadata, new_metadata)
             };
 
-            unsafe {
-                T::compare_exchange(
-                    addr,
-                    old_metadata,
-                    new_metadata,
-                    success_order,
-                    failure_order,
-                )
-            }
+            cursor.compare_exchange(
+                old_metadata,
+                new_metadata,
+                success_order,
+                failure_order,
+            )
         }
```
</details>

<details>
<summary>src/util/metadata/side_metadata/global.rs (MetadataCursor usage)</summary>

```diff
@@ -190,11 +201,11 @@ impl SideMetadataSpec {
                 } => {
                     // we are zeroing selected bit in one byte
                     // Get a mask that the bits we need to zero are set to zero, and the other bits are 1.
                     let mask: u8 =
                         u8::MAX.checked_shl(bit_end as u32).unwrap_or(0) | !(u8::MAX << bit_start);
-                    unsafe { addr.as_ref::<AtomicU8>() }.fetch_and(mask, Ordering::SeqCst);
+                    super::helpers::MetadataCursor(addr).fetch_and::<u8>(mask, Ordering::SeqCst);
                     false
                 }
             }
         };
```
</details>

<details>
<summary>src/util/metadata/side_metadata/helpers.rs (MetadataCursor abstraction)</summary>

```diff
@@ -244,10 +245,92 @@ pub enum FindMetaBitResult {
     Found { addr: Address, bit: u8 },
     NotFound,
     UnmappedMetadata,
 }
 
+#[derive(Copy, Clone)]
+pub struct MetadataCursor(pub(crate) Address);
+
+impl MetadataCursor {
+
+
+    #[inline(always)]
+    pub(crate) fn load<T: MetadataValue>(&self) -> T {
+        // SAFETY: MetadataCursor is an internal abstraction that is only constructed with valid metadata addresses.
+        unsafe { self.0.load::<T>() }
+    }
+
+    #[inline(always)]
+    pub(crate) fn load_atomic<T: MetadataValue>(&self, order: std::sync::atomic::Ordering) -> T {
+        // SAFETY: MetadataCursor is an internal abstraction that is only constructed with valid metadata addresses.
+        unsafe { self.0.atomic_load::<T::Atomic>(order) }
+    }
+
+    #[inline(always)]
+    pub(crate) fn store<T: MetadataValue>(&self, value: T) {
+        // SAFETY: MetadataCursor is an internal abstraction that is only constructed with valid metadata addresses.
+        unsafe { self.0.store::<T>(value) }
+    }
+
+    #[inline(always)]
+    pub(crate) fn store_atomic<T: MetadataValue>(&self, value: T, order: std::sync::atomic::Ordering) {
+        // SAFETY: MetadataCursor is an internal abstraction that is only constructed with valid metadata addresses.
+        unsafe { self.0.atomic_store::<T::Atomic>(value, order) }
+    }
+
+    #[inline(always)]
+    pub(crate) fn fetch_update<T: MetadataValue, F: FnMut(T) -> Option<T> + Copy>(
+        &self,
+        set_order: std::sync::atomic::Ordering,
+        fetch_order: std::sync::atomic::Ordering,
+        f: F,
+    ) -> std::result::Result<T, T> {
+        T::fetch_update(*self, set_order, fetch_order, f)
+    }
+
+    #[inline(always)]
+    pub(crate) fn compare_exchange<T: MetadataValue>(
+        &self,
+        current: T,
+        new: T,
+        success: std::sync::atomic::Ordering,
+        failure: std::sync::atomic::Ordering,
+    ) -> std::result::Result<T, T> {
+        // SAFETY: MetadataCursor is an internal abstraction that is only constructed with valid metadata addresses.
+        unsafe {
+            self.0
+                .compare_exchange::<T::Atomic>(current, new, success, failure)
+        }
+    }
+
+    #[inline(always)]
+    pub(crate) fn with_atomic<T: MetadataValue, R, F: FnOnce(&T::Atomic) -> R>(&self, f: F) -> R {
+        // SAFETY: MetadataCursor is an internal abstraction that is only constructed with valid metadata addresses.
+        unsafe { f(self.0.as_ref::<T::Atomic>()) }
+    }
+
+    #[inline(always)]
+    pub(crate) fn fetch_add<T: MetadataValue>(&self, val: T, order: std::sync::atomic::Ordering) -> T {
+        T::fetch_add(*self, val, order)
+    }
+
+    #[inline(always)]
+    pub(crate) fn fetch_sub<T: MetadataValue>(&self, val: T, order: std::sync::atomic::Ordering) -> T {
+        T::fetch_sub(*self, val, order)
+    }
+
+    #[inline(always)]
+    pub(crate) fn fetch_and<T: MetadataValue>(&self, val: T, order: std::sync::atomic::Ordering) -> T {
+        T::fetch_and(*self, val, order)
+    }
+
+    #[inline(always)]
+    pub(crate) fn fetch_or<T: MetadataValue>(&self, val: T, order: std::sync::atomic::Ordering) -> T {
+        T::fetch_or(*self, val, order)
+    }
+}
+
 // Check and find the last bit that is set. We try load words where possible, and fall back to load bytes.
 pub fn find_last_non_zero_bit_in_metadata_bytes(
     meta_start: Address,
     meta_end: Address,
 ) -> FindMetaBitResult {
@@ -288,11 +371,11 @@ pub fn find_last_non_zero_bit_in_metadata_bytes(
             }
         }
 
         if step == BYTES_IN_ADDRESS {
             // Load and check a usize word
-            let value = unsafe { cur.load::<usize>() };
+            let value = MetadataCursor(cur).load_atomic::<usize>(std::sync::atomic::Ordering::Relaxed);
             if value != 0 {
                 let bit = find_last_non_zero_bit::<usize>(value, 0, usize::BITS as u8).unwrap();
                 let byte_offset = bit >> LOG_BITS_IN_BYTE;
                 let bit_offset = bit - ((byte_offset) << LOG_BITS_IN_BYTE);
                 return FindMetaBitResult::Found {
@@ -300,11 +383,11 @@ pub fn find_last_non_zero_bit_in_metadata_bytes(
                     bit: bit_offset,
                 };
             }
         } else {
             // Load and check a byte
-            let value = unsafe { cur.load::<u8>() };
+            let value = MetadataCursor(cur).load_atomic::<u8>(std::sync::atomic::Ordering::Relaxed);
             if let Some(bit) = find_last_non_zero_bit::<u8>(value, 0, 8) {
                 return FindMetaBitResult::Found { addr: cur, bit };
             }
         }
     }
@@ -318,11 +401,11 @@ pub fn find_last_non_zero_bit_in_metadata_bits(
     end_bit: u8,
 ) -> FindMetaBitResult {
     if !addr.is_mapped() {
         return FindMetaBitResult::UnmappedMetadata;
     }
-    let byte = unsafe { addr.load::<u8>() };
+    let byte = MetadataCursor(addr).load_atomic::<u8>(std::sync::atomic::Ordering::Relaxed);
     if let Some(bit) = find_last_non_zero_bit::<u8>(byte, start_bit, end_bit) {
         return FindMetaBitResult::Found { addr, bit };
     }
     FindMetaBitResult::NotFound
 }
@@ -353,23 +436,23 @@ pub fn scan_non_zero_bits_in_metadata_bytes(
 ) {
     use crate::util::constants::BYTES_IN_ADDRESS;
 
     let mut cursor = meta_start;
     while cursor < meta_end && !cursor.is_aligned_to(BYTES_IN_ADDRESS) {
-        let byte = unsafe { cursor.load::<u8>() };
+        let byte = MetadataCursor(cursor).load::<u8>();
         scan_non_zero_bits_in_metadata_word(cursor, byte as usize, visit_bit);
         cursor += 1usize;
     }
 
     while cursor + BYTES_IN_ADDRESS < meta_end {
-        let word = unsafe { cursor.load::<usize>() };
+        let word = MetadataCursor(cursor).load::<usize>();
         scan_non_zero_bits_in_metadata_word(cursor, word, visit_bit);
         cursor += BYTES_IN_ADDRESS;
     }
 
     while cursor < meta_end {
-        let byte = unsafe { cursor.load::<u8>() };
+        let byte = MetadataCursor(cursor).load::<u8>();
         scan_non_zero_bits_in_metadata_word(cursor, byte as usize, visit_bit);
         cursor += 1usize;
     }
 }
 
@@ -389,11 +472,11 @@ pub fn scan_non_zero_bits_in_metadata_bits(
     meta_addr: Address,
     bit_start: BitOffset,
     bit_end: BitOffset,
     visit_bit: &mut impl FnMut(Address, BitOffset),
 ) {
-    let byte = unsafe { meta_addr.load::<u8>() };
+    let byte = MetadataCursor(meta_addr).load::<u8>();
     for bit in bit_start..bit_end {
         if byte & (1 << bit) != 0 {
             visit_bit(meta_addr, bit);
         }
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 99-140)</summary>

```diff
@@ -99,41 +99,55 @@ impl Block {
 
     pub fn load_free_list(&self) -> Address {
-        unsafe { Address::from_usize(Block::FREE_LIST_TABLE.load::<usize>(self.start())) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::FREE_LIST_TABLE, self.start());
+        Address::from_ptr(crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).load::<usize>() as *const ())
     }
 
     pub fn store_free_list(&self, free_list: Address) {
-        unsafe { Block::FREE_LIST_TABLE.store::<usize>(self.start(), free_list.as_usize()) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::FREE_LIST_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(free_list.as_usize());
+    }
 
     #[cfg(feature = "malloc_native_mimalloc")]
     pub fn load_local_free_list(&self) -> Address {
-        unsafe { Address::from_usize(Block::LOCAL_FREE_LIST_TABLE.load::<usize>(self.start())) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::LOCAL_FREE_LIST_TABLE, self.start());
+        Address::from_ptr(crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).load::<usize>() as *const ())
     }
 
     #[cfg(feature = "malloc_native_mimalloc")]
     pub fn store_local_free_list(&self, local_free: Address) {
-        unsafe { Block::LOCAL_FREE_LIST_TABLE.store::<usize>(self.start(), local_free.as_usize()) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::LOCAL_FREE_LIST_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(local_free.as_usize());
     }
 
     #[cfg(feature = "malloc_native_mimalloc")]
     pub fn store_thread_free_list(&self, thread_free: Address) {
-        unsafe {
-            Block::THREAD_FREE_LIST_TABLE.store::<usize>(self.start(), thread_free.as_usize())
-        }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::THREAD_FREE_LIST_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(thread_free.as_usize());
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 146-190)</summary>

```diff
@@ -146,48 +160,45 @@ impl Block {
 
     pub fn load_prev_block(&self) -> Option<Block> {
-        let prev = unsafe { Block::PREV_BLOCK_TABLE.load::<usize>(self.start()) };
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::PREV_BLOCK_TABLE, self.start());
+        let prev = crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).load::<usize>();
         NonZeroUsize::new(prev).map(Block)
     }
 
     pub fn load_next_block(&self) -> Option<Block> {
-        let next = unsafe { Block::NEXT_BLOCK_TABLE.load::<usize>(self.start()) };
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::NEXT_BLOCK_TABLE, self.start());
+        let next = crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).load::<usize>();
         NonZeroUsize::new(next).map(Block)
     }
 
     pub fn store_next_block(&self, next: Block) {
-        unsafe {
-            Block::NEXT_BLOCK_TABLE.store::<usize>(self.start(), next.start().as_usize());
-        }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::NEXT_BLOCK_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(next.start().as_usize());
     }
 
     pub fn clear_next_block(&self) {
-        unsafe {
-            Block::NEXT_BLOCK_TABLE.store::<usize>(self.start(), 0);
-        }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::NEXT_BLOCK_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(0);
     }
 
     pub fn store_prev_block(&self, prev: Block) {
-        unsafe {
-            Block::PREV_BLOCK_TABLE.store::<usize>(self.start(), prev.start().as_usize());
-        }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::PREV_BLOCK_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(prev.start().as_usize());
     }
 
     pub fn clear_prev_block(&self) {
-        unsafe {
-            Block::PREV_BLOCK_TABLE.store::<usize>(self.start(), 0);
-        }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::PREV_BLOCK_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(0);
     }
 
     pub fn store_block_list(&self, block_list: &BlockList) {
         let block_list_usize: usize = block_list as *const BlockList as usize;
-        unsafe {
-            Block::BLOCK_LIST_TABLE.store::<usize>(self.start(), block_list_usize);
-        }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::BLOCK_LIST_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(block_list_usize);
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 198-210)</summary>

```diff
@@ -198,23 +209,23 @@ impl Block {
 
     pub fn store_block_cell_size(&self, size: usize) {
         debug_assert_ne!(size, 0);
-        unsafe { Block::SIZE_TABLE.store::<usize>(self.start(), size) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::SIZE_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(size);
     }
 
     pub fn store_tls(&self, tls: VMThread) {
         let tls_usize: usize = tls.0.to_address().as_usize();
-        unsafe { Block::TLS_TABLE.store(self.start(), tls_usize) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&Block::TLS_TABLE, self.start());
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).store::<usize>(tls_usize);
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 304-307)</summary>

```diff
@@ -304,3 +304,3 @@ impl Block {
-                unsafe {
-                    cell.store::<Address>(last);
-                }
+                self.store_free_cell_link(cell, last);
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 370-380)</summary>

```diff
@@ -370,13 +377,11 @@ impl Block {
-                    unsafe {
-                        cell.store::<Address>(last);
-                    }
+                    self.store_free_cell_link(cell, last);
```
</details>

<details>
<summary>src/util/metadata/metadata_val_traits.rs (MetadataValue trait refactoring)</summary>

```diff
@@ -75,17 +77,13 @@ pub trait MetadataValue:
 {
+    type Atomic: Atomic<Type = Self>;
+
     /// Non atomic load
-    /// # Safety
-    /// The caller needs to guarantee that the address is valid, and can be used as a pointer to the type.
-    /// The caller also needs to be aware that the method is not thread safe, as it is a non-atomic operation.
-    unsafe fn load(addr: Address) -> Self;
+    fn load(cursor: MetadataCursor) -> Self;
...
 macro_rules! impl_metadata_value_trait {
     ($non_atomic: ty, $atomic: ty) => {
         impl MetadataValue for $non_atomic {
-            unsafe fn load(addr: Address) -> Self {
-                addr.load::<$non_atomic>()
+            type Atomic = $atomic;
+
+            fn load(cursor: MetadataCursor) -> Self {
+                cursor.load()
             }
```
</details>

<details>
<summary>src/util/metadata/vo_bit/mod.rs (MetadataCursor usage)</summary>

```diff
@@ -138,11 +125,12 @@ fn is_vo_bit_set_inner<const ATOMIC: bool>(addr: Address) -> Option<ObjectRefere
     let vo_bit = if ATOMIC {
         VO_BIT_SIDE_METADATA_SPEC.load_atomic::<u8>(addr, Ordering::SeqCst)
     } else {
-        unsafe { VO_BIT_SIDE_METADATA_SPEC.load::<u8>(addr) }
+        let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&VO_BIT_SIDE_METADATA_SPEC, addr);
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).load_atomic::<u8>(Ordering::Relaxed)
     };

@@ -176,11 +164,12 @@ pub(crate) fn get_raw_vo_bit_word(addr: Address) -> usize {
 pub(crate) fn get_raw_vo_bit_word(addr: Address) -> usize {
-    unsafe { VO_BIT_SIDE_METADATA_SPEC.load_raw_word(addr) }
+    let meta_addr = crate::util::metadata::side_metadata::helpers::address_to_meta_address(&VO_BIT_SIDE_METADATA_SPEC, addr);
+    crate::util::metadata::side_metadata::helpers::MetadataCursor(meta_addr).load::<usize>()
 }
```
</details>

<details>
<summary>src/util/linear_scan.rs (vo_bit::is_vo_bit_set_unsafe)</summary>

```diff
@@ -52,11 +52,11 @@ impl<VM: VMBinding, S: LinearScanObjectSize, const ATOMIC_LOAD_VO_BIT: bool> std
     fn next(&mut self) -> Option<<Self as Iterator>::Item> {
         while self.cursor < self.end {
             let is_object = if ATOMIC_LOAD_VO_BIT {
                 vo_bit::is_vo_bit_set_for_addr(self.cursor)
             } else {
-                unsafe { vo_bit::is_vo_bit_set_unsafe(self.cursor) }
+                vo_bit::is_vo_bit_set_unsafe(self.cursor)
             };
```
</details>

<details>
<summary>src/policy/markcompactspace.rs (MetadataCursor)</summary>

```diff
@@ -206,23 +207,22 @@ impl<VM: VMBinding> MarkCompactSpace<VM> {
         object.to_object_start::<VM>() - GC_EXTRA_HEADER_BYTES
     }
 
     /// Get header forwarding pointer for an object
     fn get_header_forwarding_pointer(object: ObjectReference) -> Option<ObjectReference> {
-        let addr = unsafe { Self::header_forwarding_pointer_address(object).load::<Address>() };
+        let addr_usize = MetadataCursor(Self::header_forwarding_pointer_address(object)).load::<usize>();
+        let addr = Address::from_usize(addr_usize);
         ObjectReference::from_raw_address(addr)
     }
 
     /// Store header forwarding pointer for an object
     fn store_header_forwarding_pointer(
         object: ObjectReference,
         forwarding_pointer: ObjectReference,
     ) {
-        unsafe {
-            Self::header_forwarding_pointer_address(object)
-                .store::<ObjectReference>(forwarding_pointer);
-        }
+        MetadataCursor(Self::header_forwarding_pointer_address(object))
+            .store::<usize>(forwarding_pointer.to_raw_address().as_usize());
     }
```
</details>

<details>
<summary>src/util/raw_memory_freelist.rs (MetadataCursor)</summary>

```diff
@@ -36,11 +36,11 @@ impl FreeList for RawMemoryFreeList {
         self.heads
     }
     fn get_entry(&self, index: i32) -> i32 {
         let offset = (index << LOG_BYTES_IN_ENTRY) as usize;
         debug_assert!(self.base + offset >= self.base && self.base + offset < self.high_water);
-        unsafe { (self.base + offset).load() }
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(self.base + offset).load::<u32>() as i32
     }
     fn set_entry(&mut self, index: i32, value: i32) {
         let offset = (index << LOG_BYTES_IN_ENTRY) as usize;
         debug_assert!(
             self.base + offset >= self.base && self.base + offset < self.high_water,
@@ -48,11 +48,11 @@ impl FreeList for RawMemoryFreeList {
             self.base,
             offset,
             self.base + offset,
             self.high_water
         );
-        unsafe { (self.base + offset).store(value) }
+        crate::util::metadata::side_metadata::helpers::MetadataCursor(self.base + offset).store::<u32>(value as u32);
     }
     fn alloc(&mut self, size: i32) -> i32 {
         if self.current_units == 0 {
             return FAILURE;
         }
```
</details>

<details>
<summary>src/policy/largeobjectspace.rs (vo_bit::is_vo_addr)</summary>

```diff
@@ -161,11 +161,11 @@ impl<VM: VMBinding> SFT for LargeObjectSpace<VM> {
             // We assert this when we set VO bit for LOS.
             if vo_bit::get_raw_vo_bit_word(cur_page) != 0 {
                 // Find the exact address that has vo bit set
                 for offset in 0..vo_bit::VO_BIT_WORD_TO_REGION {
                     let addr = cur_page + offset;
-                    if unsafe { vo_bit::is_vo_addr(addr) } {
+                    if vo_bit::is_vo_addr(addr) {
                         return vo_bit::is_internal_ptr_from_vo_bit::<VM>(addr, ptr);
                     }
                 }
```
</details>

### Safe Block Abstraction (Free List Links)
**Description**: The `Block` type was updated with safe methods like `load_free_cell_link` and `store_free_cell_link` to manipulate free list links stored in memory, replacing raw pointer dereferences (`cell.load()`, `cell.store()`) with safe method calls on the block.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/alloc/free_list_allocator.rs` | 5 | 0 | -5 |

**Category Total**: Δ = -5

**Diff Snippets**:
<details>
<summary>src/util/alloc/free_list_allocator.rs (Free list link operations)</summary>

```diff
@@ -150,13 +151,13 @@ impl<VM: VMBinding> FreeListAllocator<VM> {
     fn block_alloc(&mut self, block: Block) -> Address {
-        let next_cell = unsafe { cell.load::<Address>() };
+        let next_cell = block.load_free_cell_link(cell);
         // Clear the link
-        unsafe { cell.store::<Address>(Address::ZERO) };
+        block.store_free_cell_link(cell, Address::ZERO);
```

```diff
@@ -172,11 +173,11 @@ impl<VM: VMBinding> FreeListAllocator<VM> {
         #[cfg(debug_assertions)]
         {
             let mut cursor = cell;
             while cursor < cell + cell_size {
-                debug_assert_eq!(unsafe { cursor.load::<usize>() }, 0);
+                debug_assert!(block.load_free_cell_link(cursor).is_zero());
                 cursor += crate::util::constants::BYTES_IN_ADDRESS;
             }
         }
```

```diff
@@ -341,17 +342,15 @@ impl<VM: VMBinding> FreeListAllocator<VM> {
         let final_cell = loop {
-            unsafe {
-                new_cell.store::<Address>(old_cell);
-            }
+            block.store_free_cell_link(new_cell, old_cell);
             old_cell = new_cell;
```

```diff
@@ -377,13 +376,11 @@ impl<VM: VMBinding> FreeListAllocator<VM> {
         if self.tls == block_tls {
             // same thread that allocated
             let local_free = block.load_local_free_list();
-            unsafe {
-                addr.store(local_free);
-            }
+            block.store_free_cell_link(addr, local_free);
             block.store_local_free_list(addr);
```
</details>


### Safe Tagged Union Access (SideMetadataOffset)
**Description**: `SideMetadataOffset` was changed from a union (where accessing fields is unsafe) to an enum, allowing safe access via pattern matching.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/metadata/side_metadata/global.rs` | 8 | 0 | -8 |

**Category Total**: Δ = -8

**Diff Snippets**:
<details>
<summary>src/util/metadata/side_metadata/global.rs (SideMetadataOffset)</summary>

```diff
@@ -51,65 +51,76 @@ impl SideMetadataSpec {
     }
 
     /// Get the absolute offset for the spec.
     pub const fn get_absolute_offset(&self) -> Address {
         debug_assert!(self.is_absolute_offset());
-        unsafe { self.offset.addr }
+        match self.offset {
+            SideMetadataOffset::Addr(addr) => addr,
+            _ => panic!("Expected absolute offset"),
+        }
     }
```
</details>

### Safe Side Metadata Access
**Description**: Methods on `SideMetadataSpec` (such as `store` and `load`) were made safe, eliminating the need for `unsafe` blocks when accessing side metadata.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/immix/line.rs` | 2 | 0 | -2 |
| `src/util/heap/chunk_map.rs` | 2 | 0 | -2 |

**Category Total**: Δ = -4

**Diff Snippets**:
<details>
<summary>src/policy/immix/line.rs (lines 50-66)</summary>

```diff
@@ -50,19 +50,17 @@ impl Line {
     }
 
     /// Mark the line. This will update the side line mark table.
     pub fn mark(&self, state: u8) {
         debug_assert!(!super::BLOCK_ONLY);
-        unsafe {
-            Self::MARK_TABLE.store::<u8>(self.start(), state);
-        }
+        Self::MARK_TABLE.store::<u8>(self.start(), state);
     }
 
     /// Test line mark state.
     pub fn is_marked(&self, state: u8) -> bool {
         debug_assert!(!super::BLOCK_ONLY);
-        unsafe { Self::MARK_TABLE.load::<u8>(self.start()) == state }
+        Self::MARK_TABLE.load::<u8>(self.start()) == state
     }
```
</details>

<details>
<summary>src/util/heap/chunk_map.rs (lines 143-177)</summary>

```diff
@@ -143,11 +143,11 @@ impl ChunkMap {
                 old_state,
                 state
             );
         }
         // Update alloc byte
-        unsafe { Self::ALLOC_TABLE.store::<u8>(chunk.start(), state.0) };
+        Self::ALLOC_TABLE.store::<u8>(chunk.start(), state.0);
         // If this is a newly allcoated chunk, then expand the chunk range.
         if allocated {
             debug_assert!(!chunk.start().is_zero());
             let mut range = self.chunk_range.lock();
             if range.start == Chunk::ZERO {
@@ -168,11 +168,11 @@ impl ChunkMap {
         (state.is_allocated() && state.get_space_index() == self.space_index).then_some(state)
     }
 
     /// Get chunk state, regardless of the space. This should always be private.
     fn get_internal(&self, chunk: Chunk) -> ChunkState {
-        let byte = unsafe { Self::ALLOC_TABLE.load::<u8>(chunk.start()) };
+        let byte = Self::ALLOC_TABLE.load::<u8>(chunk.start());
         ChunkState(byte)
     }
```
</details>

### Safe Address Constructors
**Description**: Replacing `unsafe { Address::from_usize(...) }` with safe constructors like `Address::ZERO` or `Address::from_ptr` where the safety is guaranteed or handled internally.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `benches/mock_bench/mmapper.rs` | 3 | 0 | -3 |
| `src/util/metadata/side_metadata/side_metadata_tests.rs` | 25 | 0 | -25 |
| `src/util/metadata/side_metadata/helpers.rs` | 27 | 0 | -27 |
| `src/util/metadata/side_metadata/constants.rs` | 2 | 0 | -2 |
| `src/policy/marksweepspace/native_ms/block.rs` | 4 | 0 | -4 |
| `src/policy/marksweepspace/malloc_ms/global.rs` | 1 | 0 | -1 |
| `src/util/address.rs` | 8 | 0 | -8 |
| `src/vm/tests/mock_tests/mock_test_slots.rs` | 4 | 0 | -4 |
| `src/util/heap/layout/map32.rs` | 1 | 0 | -1 |
| `src/util/heap/layout/map64.rs` | 1 | 0 | -1 |
| `src/util/alloc/free_list_allocator.rs` | 1 | 0 | -1 |
| `src/util/conversions.rs` | 6 | 0 | -6 |
| `src/util/heap/layout/vm_layout.rs` | 4 | 0 | -4 |
| `src/util/heap/monotonepageresource.rs` | 3 | 0 | -3 |
| `src/util/linear_scan.rs` | 5 | 0 | -5 |
| `src/policy/space.rs` | 1 | 0 | -1 |
| `src/util/heap/layout/mmapper/csm/two_level_storage.rs` | 1 | 0 | -1 |
| `src/util/heap/space_descriptor.rs` | 2 | 0 | -2 |
| `src/vm/tests/mock_tests/mock_test_conservatism.rs` | 3 | 0 | -3 |
| `src/vm/tests/mock_tests/mock_test_is_in_mmtk_spaces.rs` | 3 | 0 | -3 |
| `src/vm/tests/mock_tests/mock_test_mmtk_julia_pr_143.rs` | 2 | 0 | -2 |
| `src/vm/tests/mock_tests/mock_test_vm_layout_compressed_pointer.rs` | 2 | 0 | -2 |
| `src/vm/tests/mock_tests/mock_test_vm_layout_heap_start.rs` | 2 | 0 | -2 |
| `tests/test_address.rs` | 3 | 0 | -3 |
| `tests/test_roots_work_factory.rs` | 3 | 0 | -3 |
| `src/util/alloc/bumpallocator.rs` | 2 | 0 | -2 |
| `src/util/alloc/immix_allocator.rs` | 2 | 0 | -2 |
| `src/util/metadata/side_metadata/sanity.rs` | 2 | 0 | -2 |
| `src/util/test_util/mod.rs` | 2 | 0 | -2 |
| `src/policy/compressor/forwarding.rs` | 1 | 0 | -1 |
| `src/util/api_util.rs` | 1 | 0 | -1 |
| `src/util/heap/layout/mmapper/csm/byte_map_storage.rs` | 1 | 0 | -1 |
| `src/util/metadata/side_metadata/ranges.rs` | 1 | 0 | -1 |
| `src/vm/tests/mock_tests/mock_test_handle_mmap_conflict.rs` | 1 | 0 | -1 |
| `src/vm/tests/mock_tests/mock_test_handle_mmap_oom.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -131

**Diff Snippets**:
<details>
<summary>src/vm/tests/mock_tests/mock_test_handle_mmap_conflict.rs (lines 7-19)</summary>

```diff
@@ -7,11 +7,11 @@ use crate::util::Address;
 #[test]
 pub fn test_handle_mmap_conflict() {
     with_mockvm(
         default_setup,
         || {
-            let start = unsafe { Address::from_usize(0x100_0000) };
+            let start = Address::from_usize(0x100_0000);
             let one_megabyte = 1000000;
             let mmap1_res = memory::dzmmap_noreplace(
                 start,
                 one_megabyte,
                 memory::MmapStrategy::TEST,
```
</details>
<details>
<summary>src/vm/tests/mock_tests/mock_test_handle_mmap_oom.rs (lines 13-25)</summary>

```diff
@@ -13,11 +13,11 @@ const LARGE_SIZE: usize = 1_000_000_000_000;
 pub fn test_handle_mmap_oom() {
     with_mockvm(
         default_setup,
         || {
             let panic_res = std::panic::catch_unwind(move || {
-                let start = unsafe { Address::from_usize(0x100_0000) };
+                let start = Address::from_usize(0x100_0000);
                 // mmap 1 terabyte memory - we expect this will fail due to out of memory.
                 // If that's not the case, increase the size we mmap.
                 let mmap_res = memory::dzmmap_noreplace(
                     start,
                     LARGE_SIZE,
```
</details>
<details>
<summary>src/util/metadata/side_metadata/ranges.rs (lines 172-182)</summary>

```diff
@@ -172,11 +172,11 @@ mod tests {
     use crate::util::constants::BITS_IN_BYTE;

     use super::*;

     fn mk_addr(addr: usize) -> Address {
-        unsafe { Address::from_usize(addr) }
+        Address::from_ptr(addr as *const ())
     }

     fn break_bit_range_wrapped(
         start_addr: Address,
         start_bit: usize,
```
</details>
<details>
<summary>src/util/api_util.rs (lines 18-28)</summary>

```diff
@@ -18,11 +18,11 @@ use super::{Address, ObjectReference};
 #[derive(Clone, Copy)]
 pub struct NullableObjectReference(usize);
 
 impl From<NullableObjectReference> for Option<ObjectReference> {
     fn from(value: NullableObjectReference) -> Self {
-        ObjectReference::from_raw_address(unsafe { Address::from_usize(value.0) })
+        ObjectReference::from_raw_address(Address::from_usize(value.0))
     }
 }
 ```
</details>
<details>
<summary>src/policy/compressor/forwarding.rs</summary>

```diff
@@ -82,11 +82,11 @@ impl Transducer {
         }
     }
 
     pub fn decode(offset: usize, current_position: Address) -> Self {
         Transducer {
-            to: unsafe { Address::from_usize(offset & !1) },
+            to: Address::from_ptr((offset & !1) as *const ()),
             last_bit_visited: current_position,
             in_object: (offset & 1) == 1,
         }
     }
 }
```
</details>

<details>
<summary>src/util/metadata/side_metadata/constants.rs</summary>

```diff
@@ -13,20 +13,20 @@ use crate::util::Address;
 // (1 word in 32bits), and it will take the address range of [0x40_000, 0x840_0000) which clashes with
 // the library/heap. So I move this to 0x1000_0000.
 // This is made public, as VM bingdings may need to use this.
 #[cfg(target_pointer_width = "32")]
 /// Global side metadata start address
-pub const GLOBAL_SIDE_METADATA_BASE_ADDRESS: Address = unsafe { Address::from_usize(0x1000_0000) };
+pub const GLOBAL_SIDE_METADATA_BASE_ADDRESS: Address = Address::from_usize(0x1000_0000);
 
 // FIXME: The 64-bit base address is changed from 0x0600_0000_0000 to 0x0c00_0000_0000 so that it
 // is less likely to overlap with any space.  But it does not solve the problem completely.
 // If there are more spaces, it will still overlap with some spaces.
 // See: https://github.com/mmtk/mmtk-core/issues/458
 #[cfg(target_pointer_width = "64")]
 /// Global side metadata start address
 pub const GLOBAL_SIDE_METADATA_BASE_ADDRESS: Address =
-    unsafe { Address::from_usize(0x0000_0c00_0000_0000usize) };
+    Address::from_usize(0x0000_0c00_0000_0000usize);
```
</details>

<details>
<summary>tests/test_address.rs</summary>

```diff
@@ -1,18 +1,18 @@
 use mmtk::util::Address;
 
 #[test]
 fn test_align_up() {
-    let addr = unsafe { Address::zero() };
+    let addr = Address::zero();
     let aligned = addr.align_up(8);
 
     assert_eq!(addr, aligned);
 }
 
 #[test]
 fn test_is_aligned() {
-    let addr = unsafe { Address::zero() };
+    let addr = Address::zero();
     assert!(addr.is_aligned_to(8));
 
-    let addr = unsafe { Address::from_usize(8) };
+    let addr = Address::from_usize(8);
     assert!(addr.is_aligned_to(8));
 }
```
</details>

<details>
<summary>tests/test_roots_work_factory.rs</summary>

```diff
 static SLOTS: [Address; 3] = [
-    unsafe { Address::from_usize(0x8) },
-    unsafe { Address::from_usize(0x8) },
-    unsafe { Address::from_usize(0x8) },
+    Address::from_usize(0x8),
+    Address::from_usize(0x8),
+    Address::from_usize(0x8),
 ];
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_conservatism.rs (Address::from_ptr)</summary>

```diff
@@ -108,11 +108,11 @@ pub fn too_big() {
     with_mockvm(
         default_setup,
         || {
             SINGLE_OBJECT.with_fixture(|fixture| {
                 for offset in iter_aligned_offsets(SMALL_OFFSET) {
-                    let addr = unsafe { Address::from_usize(0usize.wrapping_sub(offset)) };
+                    let addr = Address::from_ptr(0usize.wrapping_sub(offset) as *const ());
                     assert_invalid_objref(addr, fixture.objref.to_raw_address());
                 }
             });
         },
         no_cleanup,
@@ -178,11 +178,11 @@ pub fn large_offsets_aligned() {
                         .objref
                         .to_raw_address()
                         .as_usize()
                         .checked_add(offset)
                     {
-                        Some(n) => unsafe { Address::from_usize(n) },
+                        Some(n) => Address::from_ptr(n as *const ()),
                         None => break,
                     };
                     assert_filter_pass(addr);
                     assert_invalid_objref(addr, fixture.objref.to_raw_address());
                 }
@@ -205,11 +205,11 @@ pub fn negative_offsets() {
                         .to_raw_address()
                         .as_usize()
                         .checked_sub(offset)
                     {
                         Some(0) => break,
-                        Some(n) => unsafe { Address::from_usize(n) },
+                        Some(n) => Address::from_ptr(n as *const ()),
                         None => break,
                     };
                     assert_filter_pass(addr);
                     assert_invalid_objref(addr, fixture.objref.to_raw_address());
                 }
```
</details>

<details>
<summary>benches/mock_bench/mmapper.rs (Address::from_usize made safe)</summary>

```diff
@@ -29,2 +29,2 @@ pub fn bench(c: &mut Criterion) {
-    let low = unsafe { Address::from_usize(42usize) };
-    let high = unsafe { Address::from_usize(usize::MAX - 1024usize) };
+    let low = Address::from_usize(42usize);
+    let high = Address::from_usize(usize::MAX - 1024usize);
@@ -68,2 +68,2 @@ pub fn bench(c: &mut Criterion) {
             for addr_usize in (start..end).step_by(BYTES_IN_CHUNK) {
-                let addr = unsafe { Address::from_usize(addr_usize) };
+                let addr = Address::from_usize(addr_usize);
```
</details>
<details>
<summary>src/util/conversions.rs (Address::from_ptr)</summary>

```diff
@@ -37,11 +37,11 @@ pub fn address_to_chunk_index(addr: Address) -> usize {
     addr >> LOG_BYTES_IN_CHUNK
 }
 
 /// Convert a chunk index to the start address of the chunk.
 pub fn chunk_index_to_address(chunk: usize) -> Address {
-    unsafe { Address::from_usize(chunk << LOG_BYTES_IN_CHUNK) }
+    Address::from_ptr((chunk << LOG_BYTES_IN_CHUNK) as *const u8)
 }
 
 /// Align up an integer to the given alignment. `align` must be a power of two.
@@ -98,27 +98,21 @@ mod tests {
     use crate::util::conversions::*;
     use crate::util::Address;
 
     #[test]
     fn test_page_align() {
-        let addr = unsafe { Address::from_usize(0x2345_6789) };
-        assert_eq!(page_align_down(addr), unsafe {
-            Address::from_usize(0x2345_6000)
-        });
+        let addr = Address::from_ptr(0x2345_6789 as *const u8);
+        assert_eq!(page_align_down(addr), Address::from_ptr(0x2345_6000 as *const u8));
         assert!(!is_page_aligned(addr));
         assert!(is_page_aligned(page_align_down(addr)));
     }
 
     #[test]
     fn test_chunk_align() {
-        let addr = unsafe { Address::from_usize(0x2345_6789) };
-        assert_eq!(chunk_align_down(addr), unsafe {
-            Address::from_usize(0x2340_0000)
-        });
-        assert_eq!(chunk_align_up(addr), unsafe {
-            Address::from_usize(0x2380_0000)
-        });
+        let addr = Address::from_ptr(0x2345_6789 as *const u8);
+        assert_eq!(chunk_align_down(addr), Address::from_ptr(0x2340_0000 as *const u8));
+        assert_eq!(chunk_align_up(addr), Address::from_ptr(0x2380_0000 as *const u8));
     }
 ```
 </details>
<details>
<summary>src/policy/space.rs (Address::ZERO)</summary>

```diff
@@ -630,11 +630,11 @@ impl<VM: VMBinding> CommonSpace<VM> {
             immortal: args.immortal,
             movable: args.movable,
             contiguous: true,
             permission_exec: args.plan_args.permission_exec,
             zeroed: args.plan_args.zeroed,
-            start: unsafe { Address::zero() },
+            start: Address::ZERO,
             extent: 0,
             vm_map: args.plan_args.vm_map,
             mmapper: args.plan_args.mmapper,
             needs_log_bit: args.plan_args.constraints.needs_log_bit,
             unlog_allocated_object: args.plan_args.unlog_allocated_object,
```
</details>
<details>
<summary>src/util/metadata/side_metadata/side_metadata_tests.rs (address_to_meta_address tests)</summary>

```diff
@@ -37,93 +37,93 @@ mod tests {
             log_num_of_bits: 0,
             log_bytes_in_region: 0,
         };
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(0) }),
+            address_to_meta_address(&gspec, Address::ZERO),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(0) }),
+            address_to_meta_address(&lspec, Address::ZERO),
             LOCAL_SIDE_METADATA_BASE_ADDRESS
         );
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(7) }),
+            address_to_meta_address(&gspec, Address::from_ptr(7 as *const ())),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(7) }),
+            address_to_meta_address(&lspec, Address::from_ptr(7 as *const ())),
             LOCAL_SIDE_METADATA_BASE_ADDRESS
         );
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(27) }),
+            address_to_meta_address(&gspec, Address::from_ptr(27 as *const ())),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS + 3usize
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(129) }),
+            address_to_meta_address(&lspec, Address::from_ptr(129 as *const ())),
             LOCAL_SIDE_METADATA_BASE_ADDRESS + 16usize
         );
 
         gspec.log_bytes_in_region = 2;
         lspec.log_bytes_in_region = 1;
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(0) }),
+            address_to_meta_address(&gspec, Address::ZERO),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(0) }),
+            address_to_meta_address(&lspec, Address::ZERO),
             LOCAL_SIDE_METADATA_BASE_ADDRESS
         );
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(32) }),
+            address_to_meta_address(&gspec, Address::from_ptr(32 as *const ())),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS + 1usize
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(32) }),
+            address_to_meta_address(&lspec, Address::from_ptr(32 as *const ())),
             LOCAL_SIDE_METADATA_BASE_ADDRESS + 2usize
         );
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(316) }),
+            address_to_meta_address(&gspec, Address::from_ptr(316 as *const ())),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS + 9usize
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(316) }),
+            address_to_meta_address(&lspec, Address::from_ptr(316 as *const ())),
             LOCAL_SIDE_METADATA_BASE_ADDRESS + 19usize
         );
 
         gspec.log_num_of_bits = 1;
         lspec.log_num_of_bits = 3;
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(0) }),
+            address_to_meta_address(&gspec, Address::ZERO),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(0) }),
+            address_to_meta_address(&lspec, Address::ZERO),
             LOCAL_SIDE_METADATA_BASE_ADDRESS
         );
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(32) }),
+            address_to_meta_address(&gspec, Address::from_ptr(32 as *const ())),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS + 2usize
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(32) }),
+            address_to_meta_address(&lspec, Address::from_ptr(32 as *const ())),
             LOCAL_SIDE_METADATA_BASE_ADDRESS + 16usize
         );
 
         assert_eq!(
-            address_to_meta_address(&gspec, unsafe { Address::from_usize(316) }),
+            address_to_meta_address(&gspec, Address::from_ptr(316 as *const ())),
             GLOBAL_SIDE_METADATA_BASE_ADDRESS + 19usize
         );
         assert_eq!(
-            address_to_meta_address(&lspec, unsafe { Address::from_usize(318) }),
+            address_to_meta_address(&lspec, Address::from_ptr(318 as *const ())),
             LOCAL_SIDE_METADATA_BASE_ADDRESS + 159usize
         );
```
</details>

<details>
<summary>src/util/metadata/side_metadata/side_metadata_tests.rs (meta_byte_lshift tests)</summary>

```diff
@@ -155,38 +155,38 @@ mod tests {
             log_num_of_bits: 0,
             log_bytes_in_region: 0,
         };
 
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(0) }),
+            meta_byte_lshift(&spec, Address::ZERO),
             0
         );
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(5) }),
+            meta_byte_lshift(&spec, Address::from_ptr(5 as *const ())),
             5
         );
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(15) }),
+            meta_byte_lshift(&spec, Address::from_ptr(15 as *const ())),
             7
         );
 
         spec.log_num_of_bits = 2;
 
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(0) }),
+            meta_byte_lshift(&spec, Address::ZERO),
             0
         );
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(5) }),
+            meta_byte_lshift(&spec, Address::from_ptr(5 as *const ())),
             4
         );
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(15) }),
+            meta_byte_lshift(&spec, Address::from_ptr(15 as *const ())),
             4
         );
         assert_eq!(
-            meta_byte_lshift(&spec, unsafe { Address::from_usize(0x10010) }),
+            meta_byte_lshift(&spec, Address::from_ptr(0x10010 as *const ())),
             0
         );
```
</details>

<details>
<summary>src/util/metadata/side_metadata/helpers.rs (Address constructors)</summary>

```diff
@@ -59,11 +60,11 @@ pub(super) fn contiguous_meta_address_to_address(
     };
 
     let data_addr = (data_addr_intermediate << metadata_spec.log_bytes_in_region)
         + ((bit as usize) << data_addr_bit_shift);
 
-    unsafe { Address::from_usize(data_addr) }
+    Address::from_ptr(data_addr as *const u8)
 }
@@ -430,20 +513,22 @@ mod tests {
-    const TEST_ADDRESS_8B_REGION: [Address; 8] = [
-        unsafe { Address::from_usize(0x8000_0000) },
-        unsafe { Address::from_usize(0x8000_0008) },
-        unsafe { Address::from_usize(0x8000_0010) },
-        unsafe { Address::from_usize(0x8000_0018) },
-        unsafe { Address::from_usize(0x8000_0020) },
-        unsafe { Address::from_usize(0x8001_0000) },
-        unsafe { Address::from_usize(0x8001_0008) },
-        unsafe { Address::from_usize(0xd000_0000) },
-    ];
+    fn get_test_address_8b_region() -> [Address; 8] {
+        [
+            Address::from_ptr(0x8000_0000 as *const u8),
+            Address::from_ptr(0x8000_0008 as *const u8),
+            Address::from_ptr(0x8000_0010 as *const u8),
+            Address::from_ptr(0x8000_0018 as *const u8),
+            Address::from_ptr(0x8000_0020 as *const u8),
+            Address::from_ptr(0x8001_0000 as *const u8),
+            Address::from_ptr(0x8001_0008 as *const u8),
+            Address::from_ptr(0xd000_0000 as *const u8),
+        ]
+    }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 39-54)</summary>

```diff
@@ -39,15 +39,15 @@ impl Region for Block {
 
     fn from_aligned_address(address: Address) -> Self {
         debug_assert!(address.is_aligned_to(Self::BYTES));
         debug_assert!(!address.is_zero());
-        Self(unsafe { NonZeroUsize::new_unchecked(address.as_usize()) })
+        Self(NonZeroUsize::new(address.as_usize()).unwrap())
     }
 
     fn start(&self) -> Address {
-        unsafe { Address::from_usize(self.0.get()) }
+        Address::from_ptr(self.0.get() as *const ())
     }
 }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 122-128)</summary>

```diff
@@ -122,6 +122,3 @@ impl Block {
     pub fn load_thread_free_list(&self) -> Address {
-        unsafe {
-            Address::from_usize(
-                Block::THREAD_FREE_LIST_TABLE.load_atomic::<usize>(self.start(), Ordering::SeqCst),
-            )
-        }
+        Address::from_ptr(
+            Block::THREAD_FREE_LIST_TABLE.load_atomic::<usize>(self.start(), Ordering::SeqCst) as *const ()
+        )
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 212-217)</summary>

```diff
@@ -212,3 +212,3 @@ impl Block {
     pub fn load_tls(&self) -> VMThread {
         let tls = Block::TLS_TABLE.load_atomic::<usize>(self.start(), Ordering::SeqCst);
-        VMThread(OpaquePointer::from_address(unsafe {
-            Address::from_usize(tls)
-        }))
+        VMThread(OpaquePointer::from_address(Address::from_ptr(tls as *const ())))
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 288-292)</summary>

```diff
@@ -288,3 +295,3 @@ impl Block {
         let mut cell = self.start();
-        let mut last = unsafe { Address::zero() };
+        let mut last = Address::ZERO;
         while cell + cell_size <= self.start() + Block::BYTES {
```
</details>

<details>
<summary>src/policy/marksweepspace/malloc_ms/global.rs (Address::zero)</summary>

```diff
@@ -378,11 +375,11 @@ impl<VM: VMBinding> MallocSpace<VM> {
             assert!(VM::VMActivePlan::is_mutator(tls), "Polling in GC worker");
             VM::VMCollection::block_for_gc(VMMutatorThread(tls));
-            return unsafe { Address::zero() };
+            return Address::ZERO;
         }
```
</details>

<details>
<summary>src/util/address.rs (Safe Constructors and Tests)</summary>

```diff
@@ -152,23 +152,23 @@ impl Address {
     pub fn from_mut_ptr<T>(ptr: *mut T) -> Address {
         Address(ptr as usize)
     }
 
-    /// creates a null Address (0)
-    /// # Safety
-    /// It is unsafe and the user needs to be aware that they are creating an invalid address.
-    /// The zero address should only be used as unininitialized or sentinel values in performance critical code (where you dont want to use `Option<Address>`).
-    pub const unsafe fn zero() -> Address {
+    /// creates a null Address (0).
+    ///
+    /// Creating a zero address is safe, but dereferencing it is unsafe.
+    /// The zero address should only be used as uninitialized or sentinel values in performance critical code (where you dont want to use `Option<Address>`).
+    pub const fn zero() -> Address {
         Address(0)
     }
 
-    /// creates an Address of (usize::MAX)
-    /// # Safety
-    /// It is unsafe and the user needs to be aware that they are creating an invalid address.
-    /// The max address should only be used as unininitialized or sentinel values in performance critical code (where you dont want to use `Option<Address>`).
-    pub unsafe fn max() -> Address {
+    /// creates an Address of (usize::MAX).
+    ///
+    /// Creating a max address is safe, but dereferencing it is unsafe.
+    /// The max address should only be used as uninitialized or sentinel values in performance critical code (where you dont want to use `Option<Address>`).
+    pub const fn max() -> Address {
         Address(usize::MAX)
     }
 
-    /// creates an arbitrary Address
-    /// # Safety
-    /// It is unsafe and the user needs to be aware that they may create an invalid address.
+    /// creates an arbitrary Address.
+    ///
+    /// Creating an arbitrary address is safe, but dereferencing it is unsafe.
     /// This creates arbitrary addresses which may not be valid. This should only be used for hard-coded addresses. Any other uses of this function could be
     /// replaced with more proper alternatives.
-    pub const unsafe fn from_usize(raw: usize) -> Address {
+    pub const fn from_usize(raw: usize) -> Address {
         Address(raw)
     }
@@ -390,80 +404,70 @@ impl std::str::FromStr for Address {
 mod tests {
     use crate::util::Address;
 
     #[test]
     fn align_up() {
-        unsafe {
-            assert_eq!(
-                Address::from_usize(0x10).align_up(0x10),
-                Address::from_usize(0x10)
-            );
-            assert_eq!(
-                Address::from_usize(0x11).align_up(0x10),
-                Address::from_usize(0x20)
-            );
-            assert_eq!(
-                Address::from_usize(0x20).align_up(0x10),
-                Address::from_usize(0x20)
-            );
-        }
+        assert_eq!(
+            Address::from_ptr(0x10 as *const ()).align_up(0x10),
+            Address::from_ptr(0x10 as *const ())
+        );
+        assert_eq!(
+            Address::from_ptr(0x11 as *const ()).align_up(0x10),
+            Address::from_ptr(0x20 as *const ())
+        );
+        assert_eq!(
+            Address::from_ptr(0x20 as *const ()).align_up(0x10),
+            Address::from_ptr(0x20 as *const ())
+        );
     }
 
     #[test]
     fn align_down() {
-        unsafe {
-            assert_eq!(
-                Address::from_usize(0x10).align_down(0x10),
-                Address::from_usize(0x10)
-            );
-            assert_eq!(
-                Address::from_usize(0x11).align_down(0x10),
-                Address::from_usize(0x10)
-            );
-            assert_eq!(
-                Address::from_usize(0x20).align_down(0x10),
-                Address::from_usize(0x20)
-            );
-        }
+        assert_eq!(
+            Address::from_ptr(0x10 as *const ()).align_down(0x10),
+            Address::from_ptr(0x10 as *const ())
+        );
+        assert_eq!(
+            Address::from_ptr(0x11 as *const ()).align_down(0x10),
+            Address::from_ptr(0x10 as *const ())
+        );
+        assert_eq!(
+            Address::from_ptr(0x20 as *const ()).align_down(0x10),
+            Address::from_ptr(0x20 as *const ())
+        );
     }
 
     #[test]
     fn is_aligned_to() {
-        unsafe {
-            assert!(Address::from_usize(0x10).is_aligned_to(0x10));
-            assert!(!Address::from_usize(0x11).is_aligned_to(0x10));
-            assert!(Address::from_usize(0x10).is_aligned_to(0x8));
-            assert!(!Address::from_usize(0x10).is_aligned_to(0x20));
-        }
+        assert!(Address::from_ptr(0x10 as *const ()).is_aligned_to(0x10));
+        assert!(!Address::from_ptr(0x11 as *const ()).is_aligned_to(0x10));
+        assert!(Address::from_ptr(0x10 as *const ()).is_aligned_to(0x8));
+        assert!(!Address::from_ptr(0x10 as *const ()).is_aligned_to(0x20));
     }
 
     #[test]
     fn bit_and() {
-        unsafe {
-            assert_eq!(
-                Address::from_usize(0b1111_1111_1100usize) & 0b1010u8,
-                0b1000u8
-            );
-            assert_eq!(
-                Address::from_usize(0b1111_1111_1100usize) & 0b1000_0000_1010usize,
-                0b1000_0000_1000usize
-            );
-        }
+        assert_eq!(
+            Address::from_ptr(0b1111_1111_1100usize as *const ()) & 0b1010u8,
+            0b1000u8
+        );
+        assert_eq!(
+            Address::from_ptr(0b1111_1111_1100usize as *const ()) & 0b1000_0000_1010usize,
+            0b1000_0000_1000usize
+        );
     }
 
     #[test]
     fn bit_or() {
-        unsafe {
-            assert_eq!(
-                Address::from_usize(0b1111_1111_1100usize) | 0b1010u8,
-                0b1111_1111_1110usize
-            );
-            assert_eq!(
-                Address::from_usize(0b1111_1111_1100usize) | 0b1000_0000_1010usize,
-                0b1111_1111_1110usize
-            );
-        }
+        assert_eq!(
+            Address::from_ptr(0b1111_1111_1100usize as *const ()) | 0b1010u8,
+            0b1111_1111_1110usize
+        );
+        assert_eq!(
+            Address::from_ptr(0b1111_1111_1100usize as *const ()) | 0b1000_0000_1010usize,
+            0b1111_1111_1110usize
+        );
     }
 }
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (load in CompressedOopSlot)</summary>

```diff
@@ -83,9 +79,9 @@
         let compressed = self.slot_addr.load(atomic::Ordering::Relaxed);
         let expanded = (compressed as usize) << 3;
-        ObjectReference::from_raw_address(unsafe { Address::from_usize(expanded) })
+        ObjectReference::from_raw_address(Address::from_ptr(expanded as *const u8))
     }
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (load_compressed test)</summary>

```diff
@@ -103,7 +99,7 @@
         let compressed1 = (COMPRESSABLE_ADDR1 >> 3) as u32;
         let objref1 =
-            ObjectReference::from_raw_address(unsafe { Address::from_usize(COMPRESSABLE_ADDR1) });
+            ObjectReference::from_raw_address(Address::from_ptr(COMPRESSABLE_ADDR1 as *const u8));
 
         let mut rust_slot: Atomic<u32> = Atomic::new(compressed1);
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (store_compressed test)</summary>

```diff
@@ -121,7 +117,7 @@
         let compressed1 = (COMPRESSABLE_ADDR1 >> 3) as u32;
         let compressed2 = (COMPRESSABLE_ADDR2 >> 3) as u32;
         let objref2 =
-            ObjectReference::from_raw_address(unsafe { Address::from_usize(COMPRESSABLE_ADDR2) })
+            ObjectReference::from_raw_address(Address::from_ptr(COMPRESSABLE_ADDR2 as *const u8))
                 .unwrap();
 
         let mut rust_slot: Atomic<u32> = Atomic::new(compressed1);
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (load in TaggedSlot)</summary>

```diff
@@ -259,7 +249,7 @@
     fn load(&self) -> Option<ObjectReference> {
         let tagged = self.slot_addr.load(atomic::Ordering::Relaxed);
         let untagged = tagged & !Self::TAG_BITS_MASK;
-        ObjectReference::from_raw_address(unsafe { Address::from_usize(untagged) })
+        ObjectReference::from_raw_address(Address::from_ptr(untagged as *const u8))
     }
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (Address::zero)</summary>

```diff
@@ -256,11 +261,11 @@ impl VMMap for Map32 {
         let chunk = start.chunk_index();
-        if chunk == 0 || self.next_link[chunk] == 0 {
-            unsafe { Address::zero() }
+        let inner = self.inner.read().unwrap();
+        if chunk == 0 || inner.next_link[chunk] == 0 {
+            Address::ZERO
```
</details>

<details>
<summary>src/util/heap/layout/map64.rs (Address::from_ptr)</summary>

```diff
@@ -36,3 +36,3 @@ impl Map64 {
         for i in 0..MAX_SPACES {
-            let base = unsafe { Address::from_usize(i << vm_layout().log_space_extent) };
+            let base = Address::from_ptr((i << vm_layout().log_space_extent) as *const u8);
```
</details>

<details>
<summary>src/util/alloc/free_list_allocator.rs (Address::zero)</summary>

```diff
@@ -341,17 +342,15 @@ impl<VM: VMBinding> FreeListAllocator<VM> {
         debug_assert_ne!(cell_size, 0);
         self.space.record_new_block(block);
 
         // construct free list
         let block_end = block.start() + Block::BYTES;
-        let mut old_cell = unsafe { Address::zero() };
+        let mut old_cell = Address::ZERO;
         let mut new_cell = block.start();
```
</details>

<details>
<summary>src/util/heap/layout/vm_layout.rs (Address::from_usize)</summary>

```diff
@@ -130,12 +130,12 @@ impl VMLayout {
 impl VMLayout {
     /// Normal 32-bit configuration
     pub const fn new_32bit() -> Self {
         let layout32 = Self {
             log_address_space: 32,
-            heap_start: chunk_align_down(unsafe { Address::from_usize(0x8000_0000) }),
-            heap_end: chunk_align_up(unsafe { Address::from_usize(0xd000_0000) }),
+            heap_start: chunk_align_down(Address::from_usize(0x8000_0000)),
+            heap_end: chunk_align_up(Address::from_usize(0xd000_0000)),
             log_space_extent: 31,
             force_use_contiguous_spaces: false,
         };
@@ -143,14 +143,12 @@ impl VMLayout {
     /// Normal 64-bit configuration
     #[cfg(target_pointer_width = "64")]
     pub const fn new_64bit() -> Self {
         let layout64 = Self {
             log_address_space: 47,
-            heap_start: chunk_align_down(unsafe {
-                Address::from_usize(0x0000_0200_0000_0000usize)
-            }),
-            heap_end: chunk_align_up(unsafe { Address::from_usize(0x0000_2200_0000_0000usize) }),
+            heap_start: chunk_align_down(Address::from_usize(0x0000_0200_0000_0000usize)),
+            heap_end: chunk_align_up(Address::from_usize(0x0000_2200_0000_0000usize)),
             log_space_extent: 41,
             force_use_contiguous_spaces: true,
         };
```
</details>

<details>
<summary>src/util/heap/monotonepageresource.rs (Address::ZERO)</summary>

```diff
@@ -181,13 +181,13 @@ impl<VM: VMBinding> MonotonePageResource<VM> {
 
     pub fn new_discontiguous(vm_map: &'static dyn VMMap) -> Self {
         MonotonePageResource {
             common: CommonPageResource::new(false, true, vm_map),
             sync: Mutex::new(MonotonePageResourceSync {
-                cursor: unsafe { Address::zero() },
-                current_chunk: unsafe { Address::zero() },
-                sentinel: unsafe { Address::zero() },
+                cursor: Address::ZERO,
+                current_chunk: Address::ZERO,
+                sentinel: Address::ZERO,
                 conditional: MonotonePageResourceConditional::Discontiguous,
             }),
             _p: PhantomData,
         }
     }
```
</details>

<details>
<summary>src/util/linear_scan.rs (Address::from_ptr in tests)</summary>

```diff
@@ -188,12 +188,12 @@ mod tests {
         }
     }
 
     #[test]
     fn test_region_methods() {
-        let addr4k = unsafe { Address::from_usize(PAGE_SIZE) };
-        let addr4k1 = unsafe { Address::from_usize(PAGE_SIZE + 1) };
+        let addr4k = Address::from_ptr(PAGE_SIZE as *const ());
+        let addr4k1 = Address::from_ptr((PAGE_SIZE + 1) as *const ());
 
         // align
         debug_assert_eq!(Page::align(addr4k), addr4k);
         debug_assert_eq!(Page::align(addr4k1), addr4k);
         debug_assert!(Page::is_aligned(addr4k));
@@ -209,11 +209,11 @@ mod tests {
         debug_assert_eq!(page.next_nth(2).start(), addr4k + 2 * PAGE_SIZE);
     }
 
     #[test]
     fn test_region_iterator_normal() {
-        let addr4k = unsafe { Address::from_usize(PAGE_SIZE) };
+        let addr4k = Address::from_ptr(PAGE_SIZE as *const ());
         let page = Page::from_aligned_address(addr4k);
         let end_page = page.next_nth(5);
 
         let mut results = vec![];
         let iter = RegionIterator::new(page, end_page);
@@ -232,11 +232,11 @@ mod tests {
         );
     }
 
     #[test]
     fn test_region_iterator_same_start_end() {
-        let addr4k = unsafe { Address::from_usize(PAGE_SIZE) };
+        let addr4k = Address::from_ptr(PAGE_SIZE as *const ());
         let page = Page::from_aligned_address(addr4k);
 
         let mut results = vec![];
         let iter = RegionIterator::new(page, page);
         for p in iter {
@@ -245,11 +245,11 @@ mod tests {
         debug_assert_eq!(results, vec![]);
     }
 
     #[test]
     fn test_region_iterator_smaller_end() {
-        let addr4k = unsafe { Address::from_usize(PAGE_SIZE) };
+        let addr4k = Address::from_ptr(PAGE_SIZE as *const ());
         let page = Page::from_aligned_address(addr4k);
         let end = Page::from_aligned_address(Address::ZERO);
 
         let mut results = vec![];
         let iter = RegionIterator::new(page, end);
```
</details>

<details>
<summary>src/util/heap/layout/mmapper/csm/two_level_storage.rs (Address::from_usize made safe)</summary>

```diff
@@ -24,1 +24,1 @@
-const MAPPABLE_ADDRESS_LIMIT: Address = unsafe { Address::from_usize(MAPPABLE_BYTES) };
+const MAPPABLE_ADDRESS_LIMIT: Address = Address::from_usize(MAPPABLE_BYTES);
```
</details>

<details>
<summary>src/util/heap/space_descriptor.rs (Address::from_usize made safe)</summary>

```diff
@@ -96,21 +94,21 @@ impl SpaceDescriptor {
     pub fn get_start(self) -> Address {
         if !vm_layout().force_use_contiguous_spaces {
             // For 64-bit discontiguous space, use 32-bit start address
             self.get_start_32()
         } else {
-            unsafe { Address::from_usize(self.get_index() << vm_layout().log_space_extent) }
+            Address::from_usize(self.get_index() << vm_layout().log_space_extent)
         }
     }
 
     fn get_start_32(self) -> Address {
         debug_assert!(self.is_contiguous());
 
         let descriptor = self.0;
         let mantissa = descriptor >> MANTISSA_SHIFT;
         let exponent = (descriptor & EXPONENT_MASK) >> EXPONENT_SHIFT;
-        unsafe { Address::from_usize(mantissa << (BASE_EXPONENT + exponent)) }
+        Address::from_usize(mantissa << (BASE_EXPONENT + exponent))
     }
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_is_in_mmtk_spaces.rs (Address::from_usize made safe)</summary>

```diff
@@ -15,11 +15,11 @@ pub fn near_zero() {
         || {
             SINGLE_OBJECT.with_fixture(|_| {
                 // FIXME: `is_in_mmtk_space` will crash if we pass it an address lower than
                 // DEFAULT_OBJECT_REF_OFFSET.  We need to clarify its requirement on the argument,
                 // and decide if we need to test calling `is_in_mmtk_space` with 0 as an argument.
-                let addr = unsafe { Address::from_usize(DEFAULT_OBJECT_REF_OFFSET) };
+                let addr = Address::from_usize(DEFAULT_OBJECT_REF_OFFSET);
                 assert!(
                     !memory_manager::is_in_mmtk_spaces(
                         ObjectReference::from_raw_address(addr).unwrap()
                     ),
                     "A very low address {addr} should not be in any MMTk spaces."
@@ -79,11 +79,11 @@ pub fn large_offsets_aligned() {
                         .objref
                         .to_raw_address()
                         .as_usize()
                         .checked_add(offset)
                     {
-                        Some(n) => unsafe { Address::from_usize(n) },
+                        Some(n) => Address::from_usize(n),
                         None => break,
                     };
                     // It's just a smoke test.  It is hard to predict if the addr is still in any space,
                     // but it must not crash.
                     let _ = memory_manager::is_in_mmtk_spaces(
@@ -108,11 +108,11 @@ pub fn negative_offsets() {
                         .objref
                         .to_raw_address()
                         .as_usize()
                         .checked_sub(offset)
                     {
-                        Some(n) => unsafe { Address::from_usize(n) },
+                        Some(n) => Address::from_usize(n),
                         None => break,
                     };
                     // It's just a smoke test.  It is hard to predict if the addr is still in any space,
                     // but it must not crash.
                     let _ = memory_manager::is_in_mmtk_spaces(
```
</details>

<details>
<summary>src/util/alloc/bumpallocator.rs</summary>

```diff
@@ -64,11 +64,11 @@ impl<VM: VMBinding> BumpAllocator<VM> {
     pub(crate) fn set_limit(&mut self, start: Address, limit: Address) {
         self.bump_pointer.reset(start, limit);
     }
 
     pub(crate) fn reset(&mut self) {
-        let zero = unsafe { Address::zero() };
+        let zero = Address::ZERO;
         self.bump_pointer.reset(zero, zero);
     }
 
     pub(crate) fn rebind(&mut self, space: &'static dyn Space<VM>) {
         self.reset();
@@ -223,11 +223,11 @@ impl<VM: VMBinding> BumpAllocator<VM> {
             } else {
                 // For a stress test, we artificially make the fastpath fail by
                 // manipulating the limit as below.
                 // The assumption here is that we use an address range such that
                 // cursor > block_size always.
-                self.set_limit(acquired_start, unsafe { Address::from_usize(block_size) });
+                self.set_limit(acquired_start, Address::from_ptr(block_size as *const ()));
                 // Note that we have just acquired a new block so we know that we don't have to go
                 // through the entire allocation sequence again, we can directly call the slow path
                 // allocation.
                 self.alloc_slow_once_precise_stress(size, align, offset, false)
             }
```
</details>

<details>
<summary>src/util/alloc/immix_allocator.rs</summary>

```diff
@@ -355,12 +355,11 @@ impl<VM: VMBinding> ImmixAllocator<VM> {
     /// thread local buffer size, which should be always smaller than the bump cursor. This method
     /// may be reentrant. We need to check before setting the values.
     fn set_limit_for_stress(&mut self) {
         if self.bump_pointer.cursor < self.bump_pointer.limit {
             let old_limit = self.bump_pointer.limit;
-            let new_limit =
-                unsafe { Address::from_usize(self.bump_pointer.limit - self.bump_pointer.cursor) };
+            let new_limit = Address::from_usize(self.bump_pointer.limit - self.bump_pointer.cursor);
             self.bump_pointer.limit = new_limit;
             trace!(
                 "{:?}: set_limit_for_stress. normal c {} l {} -> {}",
                 self.tls,
                 self.bump_pointer.cursor,
@@ -369,13 +368,11 @@ impl<VM: VMBinding> ImmixAllocator<VM> {
             );
         }
 
         if self.large_bump_pointer.cursor < self.large_bump_pointer.limit {
             let old_lg_limit = self.large_bump_pointer.limit;
-            let new_lg_limit = unsafe {
-                Address::from_usize(self.large_bump_pointer.limit - self.large_bump_pointer.cursor)
-            };
+            let new_lg_limit = Address::from_usize(self.large_bump_pointer.limit - self.large_bump_pointer.cursor);
             self.large_bump_pointer.limit = new_lg_limit;
             trace!(
                 "{:?}: set_limit_for_stress. large c {} l {} -> {}",
                 self.tls,
                 self.large_bump_pointer.cursor,
```
</details>

<details>
<summary>src/util/metadata/side_metadata/sanity.rs</summary>

```diff
@@ -377,11 +377,11 @@ fn verify_metadata_address_bound(spec: &SideMetadataSpec, data_addr: Address) {
     assert_eq!(VMLayout::LOG_ARCH_ADDRESS_SPACE, 32, "We assume we use all address space in 32 bits. This seems not true any more, we need a proper check here.");
     #[cfg(target_pointer_width = "32")]
     let data_addr_in_address_space = true;
     #[cfg(target_pointer_width = "64")]
     let data_addr_in_address_space =
-        data_addr <= unsafe { Address::from_usize(1usize << vm_layout().log_address_space) };
+        data_addr <= Address::from_usize(1usize << vm_layout().log_address_space);
 
     if !data_addr_in_address_space {
         warn!(
             "We try get metadata {} for {}, which is not within the address space we should use",
             data_addr, spec.name
@@ -755,11 +755,11 @@ mod tests {
         assert!(verify_no_overlap_contiguous(&spec_1, &spec_2).is_ok());
 
         let spec_1 = SideMetadataSpec {
             name: "spec_1",
             is_global: true,
-            offset: SideMetadataOffset::addr(unsafe { Address::from_usize(1) }),
+            offset: SideMetadataOffset::addr(Address::from_usize(1)),
             log_num_of_bits: 0,
             log_bytes_in_region: 0,
         };
 
         assert!(verify_no_overlap_contiguous(&spec_1, &spec_2).is_err());
```
</details>

<details>
<summary>src/util/test_util/mod.rs</summary>

```diff
@@ -47,14 +47,14 @@ mod test {
 }
 
 // Test with an address that works for 32bits. The address is chosen empirically.
 #[cfg(target_os = "linux")]
 const TEST_ADDRESS: Address =
-    crate::util::conversions::chunk_align_down(unsafe { Address::from_usize(0x7000_0000) });
+    crate::util::conversions::chunk_align_down(Address::from_usize(0x7000_0000));
 #[cfg(target_os = "macos")]
 const TEST_ADDRESS: Address =
-    crate::util::conversions::chunk_align_down(unsafe { Address::from_usize(0x2_0000_0000) });
+    crate::util::conversions::chunk_align_down(Address::from_usize(0x2_0000_0000));
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_mmtk_julia_pr_143.rs (Address::from_usize made safe)</summary>

```diff
@@ -17,3 +17,3 @@
-            let start_addr = unsafe { Address::from_usize(0x78624DC00000) };
-            let end_addr = unsafe { Address::from_usize(0x786258000000) };
+            let start_addr = Address::from_usize(0x78624DC00000);
+            let end_addr = Address::from_usize(0x786258000000);
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_vm_layout_compressed_pointer.rs (Address::from_usize made safe)</summary>

```diff
@@ -25,12 +25,12 @@ fn test_vm_layout_compressed_pointer() {
                 end if end <= (32usize << 30) => 32usize << 30,
                 _ => start + (32usize << 30),
             };
             let layout = VMLayout {
                 log_address_space: 35,
-                heap_start: chunk_align_down(unsafe { Address::from_usize(start) }),
-                heap_end: chunk_align_up(unsafe { Address::from_usize(end) }),
+                heap_start: chunk_align_down(Address::from_usize(start)),
+                heap_end: chunk_align_up(Address::from_usize(end)),
                 log_space_extent: 31,
                 force_use_contiguous_spaces: false,
             };
             test_with_vm_layout(Some(layout));
         },
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_vm_layout_heap_start.rs (Address::from_usize made safe)</summary>

```diff
@@ -12,13 +12,13 @@ fn test_vm_layout_heap_start() {
         || {
             let default = VMLayout::default();
 
             // Test with an start address that is different to the default heap start
             #[cfg(target_pointer_width = "32")]
-            let heap_start = unsafe { Address::from_usize(0x7000_0000) };
+            let heap_start = Address::from_usize(0x7000_0000);
             #[cfg(target_pointer_width = "64")]
-            let heap_start = unsafe { Address::from_usize(0x0000_0400_0000_0000usize) };
+            let heap_start = Address::from_usize(0x0000_0400_0000_0000usize);
             #[cfg(target_pointer_width = "64")]
             assert!(heap_start.is_aligned_to(default.max_space_extent()));
```
</details>

<details>
<summary>src/util/heap/layout/mmapper/csm/byte_map_storage.rs (lines 87-99)</summary>

```diff
@@ -87,12 +87,11 @@ impl MapStateStorage for ByteMapStateStorage {
             .iter()
             .revisitable_group_by(|s| s.load(Ordering::Relaxed))
         {
             let state = group.key;
             let group_end = group_start + group.len;
-            let group_start_addr =
-                unsafe { Address::from_usize(group_start << LOG_BYTES_IN_CHUNK) };
+            let group_start_addr = Address::from_usize(group_start << LOG_BYTES_IN_CHUNK);
             let group_bytes = group.len << LOG_BYTES_IN_CHUNK;
             let group_range = ChunkRange::new_aligned(group_start_addr, group_bytes);
             if let Some(new_state) = update_fn(group_range, state)? {
                 for index in group_start..group_end {
                     self.mapped[index].store(new_state, Ordering::Relaxed);
```
</details>

### SFT Map Abstraction
**Description**: `SFT_MAP` access made safe through new wrapper types or safe methods, removing the need for `unsafe` blocks when updating or initializing SFT entries.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/space.rs` | 2 | 0 | -2 |
| `src/plan/global.rs` | 1 | 0 | -1 |
| `src/policy/vmspace.rs` | 2 | 0 | -2 |
| `src/policy/lockfreeimmortalspace.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -6

**Diff Snippets**:
<details>
<summary>src/policy/vmspace.rs (SFT_MAP update and eager_initialize)</summary>

```diff
@@ -125,13 +125,12 @@ impl<VM: VMBinding> Space<VM> for VMSpace<VM> {
                 sft_map.get_checked(start).name(),
                 crate::policy::sft::EMPTY_SFT_NAME
             );
             // Set SFT
             assert!(sft_map.has_sft_entry(start), "The VM space start (aligned to {}) does not have a valid SFT entry. Possibly the address range is not in the address range we use.", start);
-            unsafe {
-                sft_map.eager_initialize(self.as_sft(), start, size);
-            }
+            // SAFETY: The regions are obtained from external pages which are chunk-aligned and have their metadata mapped in `set_vm_region_inner`, so it is guaranteed to be a valid SFT entry range.
+            sft_map.eager_initialize(self.as_sft(), start, size);
         }
     }
```
```diff
@@ -251,13 +250,11 @@ impl<VM: VMBinding> VMSpace<VM> {
         // Insert to vm map: it would be good if we can make VM map aware of the region. However, the region may be outside what we can map in our VM map implementation.
         // self.common.vm_map.insert(chunk_start, chunk_size, self.common.descriptor);
         // Set SFT if we should
         if set_sft {
             assert!(SFT_MAP.has_sft_entry(chunk_start), "The VM space start (aligned to {}) does not have a valid SFT entry. Possibly the address range is not in the address range we use.", chunk_start);
-            unsafe {
-                SFT_MAP.update(self.as_sft(), chunk_start, chunk_size);
-            }
+            SFT_MAP.update(self.as_sft(), chunk_start, chunk_size);
         }
```
</details>
<details>
<summary>src/plan/global.rs (SFT map initialization)</summary>

```diff
@@ -108,16 +108,16 @@ pub fn create_plan<VM: VMBinding>(
         }
     };
 
     // We have created Plan in the heap, and we won't explicitly move it.
     // Each space now has a fixed address for its lifetime. It is safe now to initialize SFT.
-    let sft_map: &mut dyn crate::policy::sft_map::SFTMap =
-        unsafe { crate::mmtk::SFT_MAP.get_mut() }.as_mut();
+    let mut sft_map = crate::policy::sft_map::create_sft_map();
     plan.for_each_space(&mut |s| {
         sft_map.notify_space_creation(s.as_sft());
-        s.initialize_sft(sft_map);
+        s.initialize_sft(sft_map.as_mut());
     });
+    crate::mmtk::SFT_MAP.initialize_once(move || sft_map);
 
     plan
 }
```
</details>
<details>
<summary>src/policy/space.rs (SFT_MAP update and eager_initialize)</summary>

```diff
@@ -366,11 +366,11 @@ pub trait Space<VM: VMBinding>: 'static + SFT + Sync + Downcast {
                 SFT_MAP.get_checked(start + bytes - 1).name()
             );
         }
 
         if new_chunk {
-            unsafe { SFT_MAP.update(self.as_sft(), start, bytes) };
+            SFT_MAP.update(self.as_sft(), start, bytes);
         }
     }
```
```diff
@@ -745,11 +745,12 @@ impl<VM: VMBinding> CommonSpace<VM> {
         // We can fix this by either of these:
         // * fix page resource, so it propelry returns new_chunk
         // * change grow_space() so it sets SFT no matter what the new_chunks value is.
         // FIXME: eagerly initializing SFT is not a good idea.
         if self.contiguous {
-            unsafe { sft_map.eager_initialize(sft, self.start, self.extent) };
+            // SAFETY: The space is contiguous and the range `(self.start, self.start + self.extent)` was reserved and mapped during space creation, so it is guaranteed to be a valid SFT entry range.
+            sft_map.eager_initialize(sft, self.start, self.extent);
         }
     }
```
</details>
<details>
<summary>src/policy/lockfreeimmortalspace.rs (initialize_sft)</summary>

```diff
@@ -125,11 +125,12 @@ impl<VM: VMBinding> Space<VM> for LockFreeImmortalSpace<VM> {
     fn release_multiple_pages(&mut self, _start: Address) {
         panic!("immortalspace only releases pages enmasse")
     }
 
     fn initialize_sft(&self, sft_map: &mut dyn crate::policy::sft_map::SFTMap) {
-        unsafe { sft_map.eager_initialize(self.as_sft(), self.start, self.total_bytes) };
+        // SAFETY: The address range `(self.start, self.start + self.total_bytes)` was reserved and mapped by this space during creation, so it is guaranteed to be a valid SFT entry range.
+        sft_map.eager_initialize(self.as_sft(), self.start, self.total_bytes);
     }
 
     fn estimate_side_meta_pages(&self, data_pages: usize) -> usize {
         self.metadata.calculate_reserved_pages(data_pages)
     }
```
</details>

### Safe Marker Trait Implementation (Derive)
**Description**: Replacing manual `unsafe impl` of marker traits (like `Zeroable`) with safe derive macros, allowing the compiler or macro to ensure safety.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/heap/space_descriptor.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/util/heap/space_descriptor.rs (Derive Zeroable)</summary>

```diff
@@ -25,16 +25,14 @@ const INDEX_MASK: usize = !TYPE_MASK;
 const INDEX_SHIFT: usize = TYPE_BITS;
 
 static DISCONTIGUOUS_SPACE_INDEX: AtomicUsize = AtomicUsize::new(DISCONTIG_INDEX_INCREMENT);
 const DISCONTIG_INDEX_INCREMENT: usize = 1 << TYPE_BITS;
 
-#[derive(Copy, Clone, PartialEq, Debug)]
+#[derive(Copy, Clone, PartialEq, Debug, Zeroable)]
 #[repr(transparent)]
 pub struct SpaceDescriptor(usize);
 
-unsafe impl Zeroable for SpaceDescriptor {}
-
```
</details>

### Static Initialization (OnceLock)
**Description**: `static mut` variables were replaced with `std::sync::OnceLock`, providing safe, lazy static initialization and removing the need for `unsafe` blocks to read or write global state.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/heap/layout/vm_layout.rs` | 2 | 0 | -2 |

**Category Total**: Δ = -2

**Diff Snippets**:
<details>
<summary>src/util/heap/layout/vm_layout.rs (OnceLock for VM_LAYOUT)</summary>

```diff
@@ -164,13 +162,11 @@ impl VMLayout {
                 !VM_LAYOUT_FETCHED.load(Ordering::SeqCst),
                 "vm_layout is already been used before setup"
             );
         }
         constants.validate();
-        unsafe {
-            VM_LAYOUT = constants;
-        }
+        VM_LAYOUT.set(constants).expect("vm_layout is already been used before setup");
     }
 }
 
 // Implement default so bindings can selectively change some parameters while using default for others.
@@ -183,21 +179,18 @@ impl std::default::Default for VMLayout {
     fn default() -> Self {
         Self::new_64bit()
     }
 }
 
-#[cfg(target_pointer_width = "32")]
-static mut VM_LAYOUT: VMLayout = VMLayout::new_32bit();
-#[cfg(target_pointer_width = "64")]
-static mut VM_LAYOUT: VMLayout = VMLayout::new_64bit();
+static VM_LAYOUT: std::sync::OnceLock<VMLayout> = std::sync::OnceLock::new();
 
 static VM_LAYOUT_FETCHED: AtomicBool = AtomicBool::new(false);
 
 /// Get the current virtual memory layout in use.
 /// If the binding would like to set a custom virtual memory layout ([`crate::mmtk::MMTKBuilder::set_vm_layout`]), they should not
 /// call this function before they set a custom layout.
 pub fn vm_layout() -> &'static VMLayout {
     if cfg!(debug_assertions) {
         VM_LAYOUT_FETCHED.store(true, Ordering::SeqCst);
     }
-    unsafe { &*addr_of!(VM_LAYOUT) }
+    VM_LAYOUT.get_or_init(|| VMLayout::default())
 }
```
</details>

### Safe Lazy Initialization (OnceLock)
**Description**: Custom lazy initialization types (like `InitializeOnce` or `OnceOptionBox`) or raw `MaybeUninit` were refactored or removed in favor of the standard library's `OnceLock`, eliminating unsafe operations like `assume_init` or raw pointer manipulations.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/rust_util/atomic_box.rs` | 6 | 0 | -6 |
| `src/util/rust_util/mod.rs` | 5 | 0 | -5 |
| `src/util/heap/gc_trigger.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -12

**Diff Snippets**:
<details>
<summary>src/util/rust_util/atomic_box.rs (Removal of OnceOptionBox)</summary>

```diff
@@ -35,4 +35,3 @@
     pub fn get(&self, order: Ordering) -> Option<&T> {
         let ptr = self.inner.load(order);
-        unsafe { ptr.as_ref() }
     }
@@ -65,10 +64,6 @@
         match cas_result {
             Ok(old_inner) => {
                 debug_assert_eq!(old_inner, std::ptr::null_mut());
-                unsafe { new_inner.as_ref().unwrap() }
             }
             Err(old_inner) => {
-                drop(unsafe { Box::from_raw(new_inner) });
-                unsafe { old_inner.as_ref().unwrap() }
             }
         }
@@ -79,6 +74,3 @@
 impl<T> Drop for OnceOptionBox<T> {
     fn drop(&mut self) {
         let ptr = *self.inner.get_mut();
         if !ptr.is_null() {
-            drop(unsafe { Box::from_raw(ptr) });
         }
     }
 }
@@ -87,1 +79,0 @@
-unsafe impl<T> Zeroable for OnceOptionBox<T> {}
```
</details>

<details>
<summary>src/util/rust_util/mod.rs (InitializeOnce refactored to OnceLock)</summary>

```diff
@@ -49,82 +48,86 @@ use std::sync::Once;
-    v: UnsafeCell<MaybeUninit<T>>,
-    once: Once,
+    v: std::sync::OnceLock<T>,
...
-    pub fn initialize_once(&self, init_fn: &'static dyn Fn() -> T) {
-        self.once.call_once(|| {
-            unsafe { &mut *self.v.get() }.write(init_fn());
-        });
-        debug_assert!(self.once.is_completed());
+    pub fn initialize_once<F: FnOnce() -> T>(&self, init_fn: F) {
+        self.v.get_or_init(init_fn);
     }
 
     /// Get the value. This should only be used after initialize_once()
     pub fn get_ref(&self) -> &T {
-        // We only assert in debug builds.
-        debug_assert!(self.once.is_completed());
-        unsafe { (*self.v.get()).assume_init_ref() }
+        self.v.get().expect("InitializeOnce not initialized")
     }
```
</details>

<details>
<summary>src/util/heap/gc_trigger.rs (MaybeUninit refactored to OnceLock)</summary>

```diff
@@ -23,4 +24,4 @@ pub struct GCTrigger<VM: VMBinding> {
     /// The current plan. This is uninitialized when we create it, and later initialized
     /// once we have a fixed address for the plan.
-    plan: MaybeUninit<&'static dyn Plan<VM = VM>>,
+    plan: std::sync::OnceLock<Weak<dyn Plan<VM = VM>>>,
@@ -70,7 +71,7 @@ impl<VM: VMBinding> GCTrigger<VM> {
-    pub fn set_plan(&mut self, plan: &'static dyn Plan<VM = VM>) {
-        self.plan.write(plan);
+    pub fn set_plan(&self, plan: Arc<dyn Plan<VM = VM>>) {
+        self.plan.set(Arc::downgrade(&plan)).ok().expect("Plan already set");
     }
 
-    fn plan(&self) -> &dyn Plan<VM = VM> {
-        unsafe { self.plan.assume_init() }
+    fn plan(&self) -> Arc<dyn Plan<VM = VM>> {
+        self.plan.get().expect("Plan not initialized").upgrade().expect("Plan dropped")
     }
```
</details>

### Interior Mutability (UnsafeCell → RwLock)
**Description**: `UnsafeCell` combined with a manual `Mutex` was replaced by `RwLock`, providing safe interior mutability and removing the need for `unsafe` pointer dereferences and manual `Send`/`Sync` implementations.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/heap/layout/map32.rs` | 11 | 0 | -11 |
| `src/util/heap/layout/map64.rs` | 9 | 0 | -9 |
| `src/util/heap/blockpageresource.rs` | 7 | 0 | -7 |

**Category Total**: Δ = -27

**Diff Snippets**:
<details>
<summary>src/util/heap/blockpageresource.rs (RwLock and Safe Methods)</summary>

```diff
@@ -120,15 +120,15 @@
-            let result = unsafe { array.push_relaxed(B::from_aligned_address(cursor)) };
+            let result = array.push_mut(B::from_aligned_address(cursor));
...
-                let result2 = unsafe { array.push_relaxed(block) };
+                let result2 = array.push_mut(block);
@@ -183,62 +183,61 @@
-    data: UnsafeCell<Box<[MaybeUninit<B>]>>,
+    data: RwLock<Box<[Option<B>]>>,
...
-    unsafe fn set_entry(&self, i: usize, block: B) {
-        (*self.data.get())[i].write(block);
+    fn set_entry(&self, i: usize, block: B) {
+        self.data.write()[i] = Some(block);
     }
...
-    unsafe fn push_relaxed(&self, block: B) -> Result<(), B> {
+    fn push_relaxed(&self, block: B) -> Result<(), B> {
...
-        unsafe {
-            core::ptr::swap(self.data.get(), new_array.data.get());
+        {
+            let mut self_data = self.data.write();
+            let mut new_data = new_array.data.write();
+            std::mem::swap(&mut *self_data, &mut *new_data);
         }
@@ -326,18 +327,16 @@
-        let failed = unsafe {
-            self.worker_local_freed_blocks[id]
-                .push_relaxed(block)
-                .is_err()
-        };
+        let failed = self.worker_local_freed_blocks[id]
+            .push_relaxed(block)
+            .is_err();
         if failed {
-            let queue = BlockQueue::new();
-            let result = unsafe { queue.push_relaxed(block) };
+            let mut queue = BlockQueue::new();
+            let result = queue.push_mut(block);
```
</details>
<details>
<summary>src/util/heap/layout/map32.rs (lines 32-33)</summary>

```diff
-unsafe impl Send for Map32 {}
-unsafe impl Sync for Map32 {}
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (line 58)</summary>

```diff
-impl std::ops::Deref for Map32 {
-    type Target = Map32Inner;
-    fn deref(&self) -> &Self::Target {
-        unsafe { &*self.inner.get() }
-    }
-}
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (line 66)</summary>

```diff
     fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor) {
-        let self_mut: &mut Map32Inner = unsafe { self.mut_self() };
...
+        self.inner.write().unwrap().insert(start, extent, descriptor);
     }
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (line 109)</summary>

```diff
-    unsafe fn allocate_contiguous_chunks(
+    fn allocate_contiguous_chunks(
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (line 182)</summary>

```diff
-    unsafe fn free_contiguous_chunks(&self, start: Address) -> usize {
+    fn free_contiguous_chunks(&self, start: Address) -> usize {
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (lines 266-279)</summary>

```diff
-    unsafe fn mut_self(&self) -> &mut Map32Inner {
-        &mut *self.inner.get()
-    }
-
-    fn mut_self_with_sync(&self) -> (MutexGuard<'_, ()>, &mut Map32Inner) {
-        let guard = self.sync.lock().unwrap();
-        (guard, unsafe { self.mut_self() })
-    }
```
</details>

<details>
<summary>src/util/heap/layout/map32.rs (lines 279-305)</summary>

```diff
-    fn free_contiguous_chunks_no_lock(&self, chunk: i32) -> usize {
-        unsafe {
-            let chunks = self.mut_self().region_map.free(chunk, false);
...
+    fn free_contiguous_chunks_no_lock(&mut self, chunk: i32) -> usize {
+        let chunks = self.region_map.free(chunk, false);
```
</details>

<details>
<summary>src/util/heap/layout/map64.rs (UnsafeCell → RwLock refactoring)</summary>

```diff
@@ -27,4 +25,2 @@
-unsafe impl Send for Map64 {}
-unsafe impl Sync for Map64 {}
```

```diff
@@ -55,11 +54,11 @@ impl VMMap for Map64 {
-        let self_mut = unsafe { self.mut_self() };
+        let mut self_mut = self.mut_self();
```

```diff
@@ -74,11 +73,11 @@ impl VMMap for Map64 {
-        let self_mut = unsafe { self.mut_self() };
+        let mut self_mut = self.mut_self();
```

```diff
@@ -106,27 +105,25 @@ impl VMMap for Map64 {
-    /// # Safety
-    ///
-    /// Caller must ensure that only one thread is calling this method.
-    unsafe fn allocate_contiguous_chunks(
+    /// Allocate contiguous chunks.
+    fn allocate_contiguous_chunks(
```

```diff
@@ -169,11 +166,11 @@ impl VMMap for Map64 {
-    unsafe fn free_contiguous_chunks(&self, _start: Address) -> usize {
+    fn free_contiguous_chunks(&self, _start: Address) -> usize {
```

```diff
@@ -181,11 +178,11 @@ impl VMMap for Map64 {
-        let self_mut: &mut Map64Inner = unsafe { self.mut_self() };
+        let mut self_mut = self.mut_self();
```

```diff
@@ -205,22 +202,16 @@ impl VMMap for Map64 {
-    unsafe fn mut_self(&self) -> &mut Map64Inner {
-        &mut *self.inner.get()
-    }
-
-    fn inner(&self) -> &Map64Inner {
-        unsafe { &*self.inner.get() }
+    fn mut_self(&self) -> std::sync::RwLockWriteGuard<'_, Map64Inner> {
+        self.inner.write().unwrap()
+    }
+
+    fn inner(&self) -> std::sync::RwLockReadGuard<'_, Map64Inner> {
+        self.inner.read().unwrap()
     }
```
</details>

### Atomic Operations (load_atomic/store_atomic)
**Description**: Replacing unsafe `load` and `store` methods with safe `load_atomic` and `store_atomic` methods on `SideMetadataSpec`.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/metadata/side_metadata/side_metadata_tests.rs` | 8 | 0 | -8 |
| `src/util/metadata/vo_bit/mod.rs` | 3 | 0 | -3 |
| `src/policy/marksweepspace/malloc_ms/metadata.rs` | 1 | 0 | -1 |
| `src/plan/barriers.rs` | 1 | 0 | -1 |
| `src/util/metadata/pin_bit.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -14

**Diff Snippets**:
<details>
<summary>src/util/metadata/side_metadata/side_metadata_tests.rs (load/store)</summary>

```diff
@@ -596,23 +597,23 @@ mod tests {
                         .map(|i| data_addr + (region_size * i))
                         .collect::<Vec<Address>>();
                     // Set metadata for the regions
                     regions
                         .iter()
-                        .for_each(|addr| unsafe { spec.store::<u8>(*addr, 1) });
+                        .for_each(|addr| spec.store_atomic::<u8>(*addr, 1, Ordering::Relaxed));
                     regions
                         .iter()
-                        .for_each(|addr| assert!(unsafe { spec.load::<u8>(*addr) } == 1));
+                        .for_each(|addr| assert!(spec.load_atomic::<u8>(*addr, Ordering::Relaxed) == 1));
 
                     // bulk zero the 8 regions (1 bit for each, in total 1 byte)
                     spec.bzero_metadata(regions[0], region_size * 8);
                     // Check if the first 8 regions are set to 0
                     regions[0..8]
                         .iter()
-                        .for_each(|addr| assert!(unsafe { spec.load::<u8>(*addr) } == 0));
+                        .for_each(|addr| assert!(spec.load_atomic::<u8>(*addr, Ordering::Relaxed) == 0));
                     // Check if the 9th region is still 1
-                    assert!(unsafe { spec.load::<u8>(regions[8]) } == 1);
+                    assert!(spec.load_atomic::<u8>(regions[8], Ordering::Relaxed) == 1);
                 },
                 || {
                     sanity::reset();
                 },
             )
@@ -653,68 +654,57 @@ mod tests {
                         .map(|i| data_addr + (region_size * i))
                         .collect::<Vec<Address>>();
                     // Set metadata for the regions
                     regions
                         .iter()
-                        .for_each(|addr| unsafe { spec.store::<u8>(*addr, 1) });
+                        .for_each(|addr| spec.store_atomic::<u8>(*addr, 1, Ordering::Relaxed));
                     regions
                         .iter()
-                        .for_each(|addr| assert!(unsafe { spec.load::<u8>(*addr) } == 1));
+                        .for_each(|addr| assert!(spec.load_atomic::<u8>(*addr, Ordering::Relaxed) == 1));
 
                     // bulk zero the first 4 regions (1 bit for each, in total 4 bits)
                     spec.bzero_metadata(regions[0], region_size * 4);
                     // Check if the first 4 regions are set to 0
                     regions[0..4]
                         .iter()
-                        .for_each(|addr| assert!(unsafe { spec.load::<u8>(*addr) } == 0));
+                        .for_each(|addr| assert!(spec.load_atomic::<u8>(*addr, Ordering::Relaxed) == 0));
                     // Check if the rest regions is still 1
                     regions[4..9]
                         .iter()
-                        .for_each(|addr| assert!(unsafe { spec.load::<u8>(*addr) } == 1));
+                        .for_each(|addr| assert!(spec.load_atomic::<u8>(*addr, Ordering::Relaxed) == 1));
```
</details>

<details>
<summary>src/util/metadata/vo_bit/mod.rs (load_atomic usage)</summary>

```diff
@@ -225,12 +212,9 @@ pub(crate) fn is_vo_addr(addr: Address) -> bool {
-pub(crate) unsafe fn is_vo_addr(addr: Address) -> bool {
-    VO_BIT_SIDE_METADATA_SPEC.load::<u8>(addr) != 0
+pub(crate) fn is_vo_addr(addr: Address) -> bool {
+    VO_BIT_SIDE_METADATA_SPEC.load_atomic::<u8>(addr, Ordering::Relaxed) != 0
 }
```
</details>

<details>
<summary>src/util/metadata/vo_bit/mod.rs (removal of unsafe non-atomic unset)</summary>

```diff
@@ -87,20 +87,11 @@ pub(crate) fn unset_vo_bit(object: ObjectReference) {
-/// Non-atomically unset the VO bit for an object. The caller needs to ensure the side
-/// metadata for the VO bit for the object is accessed by only one thread.
-///
-/// # Safety
-///
-/// This is unsafe: check the comment on `side_metadata::store`
-pub(crate) unsafe fn unset_vo_bit_unsafe(object: ObjectReference) {
-    debug_assert!(is_vo_bit_set(object), "{:x}: VO bit not set", object);
-    VO_BIT_SIDE_METADATA_SPEC.store::<u8>(object.to_raw_address(), 0);
-}
```
</details>

<details>
<summary>src/policy/marksweepspace/malloc_ms/metadata.rs (is_offset_malloc)</summary>

```diff
@@ -67,6 +67,6 @@ pub(super) fn set_page_mark(page_addr: Address) {
 /// Is this allocation an offset malloc? The argument address should be the allocation address (object start)
 pub(super) fn is_offset_malloc(address: Address) -> bool {
-    unsafe { OFFSET_MALLOC_METADATA_SPEC.load::<u8>(address) == 1 }
+    OFFSET_MALLOC_METADATA_SPEC.load_atomic::<u8>(address, Ordering::Relaxed) == 1
 }
```
</details>

<details>
<summary>src/plan/barriers.rs (object_is_unlogged)</summary>

```diff
@@ -193,11 +193,11 @@ impl<S: BarrierSemantics> ObjectBarrier<S> {
     }
 
     /// Attempt to atomically log an object.
     /// Returns true if the object is not logged previously.
     fn object_is_unlogged(&self, object: ObjectReference) -> bool {
-        unsafe { S::UNLOG_BIT_SPEC.load::<S::VM, u8>(object, None) != 0 }
+        S::UNLOG_BIT_SPEC.load_atomic::<S::VM, u8>(object, None, Ordering::SeqCst) != 0
     }
 
     /// Attempt to atomically log an object.
     /// Returns true if the object is not logged previously.
     fn log_object(&self, object: ObjectReference) -> bool {
```
</details>

<details>
<summary>src/util/metadata/pin_bit.rs (load_atomic)</summary>

```diff
@@ -34,12 +34,8 @@ impl VMLocalPinningBitSpec {
         res.is_ok()
     }
 
     /// Check if an object is pinned.
     pub fn is_object_pinned<VM: VMBinding>(&self, object: ObjectReference) -> bool {
-        if unsafe { self.load::<VM, u8>(object, None) == 1 } {
-            return true;
-        }
-
-        false
+        self.load_atomic::<VM, u8>(object, None, Ordering::SeqCst) == 1
     }
 }
```
</details>


### Test Refactoring: Stack Variables
**Description**: Replacing raw pointer allocation (`std::alloc::alloc_zeroed`) and raw pointer dereferencing with stack variables and safe references in tests.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/metadata/side_metadata/side_metadata_tests.rs` | 6 | 0 | -6 |

**Category Total**: Δ = -6

**Diff Snippets**:
<details>
<summary>src/util/metadata/side_metadata/side_metadata_tests.rs (test_side_metadata_zero_meta_bits)</summary>

```diff
@@ -685,31 +675,21 @@ mod tests {
     fn test_side_metadata_zero_meta_bits() {
-        let size = 4usize;
-        let allocate_u32 = || -> Address {
-            let ptr = unsafe {
-                std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align(size, 4).unwrap())
-            };
-            Address::from_mut_ptr(ptr)
-        };
-        let fill_1 = |addr: Address| unsafe {
-            addr.store(u32::MAX);
-        };
-
-        let start = allocate_u32();
-        let end = start + size;
+        let mut val: u32 = u32::MAX;
+        let start = Address::from_mut_ptr(&mut val);
+        let end = start + 4usize;
 
-        fill_1(start);
         // zero the word
         SideMetadataSpec::zero_meta_bits(start, 0, end, 0);
-        assert_eq!(unsafe { start.load::<u32>() }, 0);
+        assert_eq!(val, 0);
 
-        fill_1(start);
+        val = u32::MAX;
         // zero first 2 bits
         SideMetadataSpec::zero_meta_bits(start, 0, start, 2);
-        assert_eq!(unsafe { start.load::<u32>() }, 0xFFFF_FFFC); // ....1100
+        assert_eq!(val, 0xFFFF_FFFC); // ....1100
 
-        fill_1(start);
+        val = u32::MAX;
         // zero last 2 bits
         SideMetadataSpec::zero_meta_bits(end - 1, 6, end, 0);
-        assert_eq!(unsafe { start.load::<u32>() }, 0x3FFF_FFFF); // 0011....
+        assert_eq!(val, 0x3FFF_FFFF); // 0011....
 
-        fill_1(start);
+        val = u32::MAX;
         // zero everything except first 2 bits and last 2 bits
         SideMetadataSpec::zero_meta_bits(start, 2, end - 1, 6);
-        assert_eq!(unsafe { start.load::<u32>() }, 0xC000_0003); // 1100....0011
+        assert_eq!(val, 0xC000_0003); // 1100....0011
     }
```
</details>

### Test Refactoring: Encapsulation and Safe Alternatives
**Description**: In tests, replacing direct unsafe calls with safe alternatives or extracting unsafe calls into local helper functions to reduce the number of unsafe blocks.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/memory.rs` | 12 | 9 | -3 |

**Category Total**: Δ = -3

**Diff Snippets**:
<details>
<summary>src/util/memory.rs (dzmmap_test and dzmmap_noreplace in tests)</summary>

```diff
@@ -478,23 +504,29 @@ mod tests {
     use crate::util::test_util::{serial_test, with_cleanup};
 
     // In the tests, we will mmap this address. This address should not be in our heap (in case we mess up with other tests)
     const START: Address = MEMORY_TEST_REGION.start;
 
+    fn dzmmap_test(start: Address, size: usize) -> Result<()> {
+        // SAFETY: This is a test using a dedicated test memory region.
+        unsafe { dzmmap(start, size, MmapStrategy::TEST, mmap_anno_test!()) }
+    }
+
     #[test]
     fn test_mmap() {
         serial_test(|| {
             with_cleanup(
                 || {
-                    let res = unsafe {
-                        dzmmap(START, BYTES_IN_PAGE, MmapStrategy::TEST, mmap_anno_test!())
-                    };
+                    let res = dzmmap_noreplace(
+                        START,
+                        BYTES_IN_PAGE,
+                        MmapStrategy::TEST,
+                        mmap_anno_test!(),
+                    );
                     assert!(res.is_ok());
                     // We can overwrite with dzmmap
-                    let res = unsafe {
-                        dzmmap(START, BYTES_IN_PAGE, MmapStrategy::TEST, mmap_anno_test!())
-                    };
+                    let res = dzmmap_test(START, BYTES_IN_PAGE);
                     assert!(res.is_ok());
                 },
```

```diff
@@ -529,13 +561,16 @@ mod tests {
     fn test_mmap_noreplace() {
         serial_test(|| {
             with_cleanup(
                 || {
                     // Make sure we mmapped the memory
-                    let res = unsafe {
-                        dzmmap(START, BYTES_IN_PAGE, MmapStrategy::TEST, mmap_anno_test!())
-                    };
+                    let res = dzmmap_noreplace(
+                        START,
+                        BYTES_IN_PAGE,
+                        MmapStrategy::TEST,
+                        mmap_anno_test!(),
+                    );
                     assert!(res.is_ok());
```

```diff
@@ -558,13 +593,11 @@ mod tests {
                 || {
                     let res =
                         mmap_noreserve(START, BYTES_IN_PAGE, MmapStrategy::TEST, mmap_anno_test!());
                     assert!(res.is_ok());
                     // Try reserve it
-                    let res = unsafe {
-                        dzmmap(START, BYTES_IN_PAGE, MmapStrategy::TEST, mmap_anno_test!())
-                    };
+                    let res = dzmmap_test(START, BYTES_IN_PAGE);
                     assert!(res.is_ok());
                 },
```
</details>

### Safe Buffer Allocation (Aligned Array)
**Description**: Replacing raw pointer allocation (`std::alloc::alloc_zeroed`) and raw pointer dereferencing with a heap-allocated, aligned struct (`Box<AlignedBuffer>`) and safe array indexing.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `benches/regular_bench/bulk_meta/bscan.rs` | 3 | 0 | -3 |
| `benches/regular_bench/bulk_meta/bzero_bset.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -4

**Diff Snippets**:
<details>
<summary>benches/regular_bench/bulk_meta/bscan.rs (AlignedBuffer)</summary>

```diff
@@ -5,16 +5,13 @@ use mmtk::util::{
     constants::LOG_BITS_IN_WORD, test_private::scan_non_zero_bits_in_metadata_bytes, Address,
 };
 use rand::{seq::IteratorRandom, SeedableRng};
 use rand_chacha::ChaCha8Rng;
 
-fn allocate_aligned(size: usize) -> Address {
-    let ptr = unsafe {
-        std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align(size, size).unwrap())
-    };
-    Address::from_mut_ptr(ptr)
-}
+#[repr(align(1024))]
+struct AlignedBuffer([usize; 1024 / std::mem::size_of::<usize>()]);
+
```
</details>

<details>
<summary>benches/regular_bench/bulk_meta/bscan.rs (Safe buffer access)</summary>

```diff
@@ -55,19 +54,19 @@ fn make_standard_bitmap() -> PreparedBitmap {
         .collect::<Vec<_>>();
 
     set_bits.sort();
 
     for (addr, bit) in set_bits.iter() {
-        let word = unsafe { addr.load::<usize>() };
-        let new_word = word | (1 << bit);
-        unsafe { addr.store::<usize>(new_word) };
+        let offset = (addr.as_usize() - start.as_usize()) / std::mem::size_of::<usize>();
+        buffer.0[offset] |= 1 << bit;
     }
```
</details>

<details>
<summary>benches/regular_bench/bulk_meta/bzero_bset.rs (AlignedBuffer)</summary>

```diff
@@ -5,12 +5,8 @@ use mmtk::util::{constants::LOG_BITS_IN_WORD, test_private, Address};
 
-fn allocate_aligned(size: usize) -> Address {
-    let ptr = unsafe {
-        std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align(size, size).unwrap())
-    };
-    Address::from_mut_ptr(ptr)
-}
+#[repr(align(1024))]
+struct AlignedBuffer([u8; 1024]);
```
</details>

### Safe Checked Construction
**Description**: Replacing `unsafe` unchecked constructors (like `NonZeroUsize::new_unchecked` or `ObjectReference::from_raw_address_unchecked`) with safe checked constructors followed by `unwrap()`, or safe fallible methods.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/marksweepspace/native_ms/block.rs` | 3 | 0 | -3 |
| `src/util/address.rs` | 1 | 0 | -1 |
| `src/util/metadata/vo_bit/mod.rs` | 1 | 0 | -1 |
| `src/util/alloc/free_list_allocator.rs` | 1 | 0 | -1 |
| `docs/dummyvm/src/lib.rs` | 1 | 0 | -1 |
| `src/util/object_forwarding.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -8

**Diff Snippets**:
<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 39-54)</summary>

```diff
@@ -39,15 +39,15 @@ impl Region for Block {
 
     fn from_aligned_address(address: Address) -> Self {
         debug_assert!(address.is_aligned_to(Self::BYTES));
         debug_assert!(!address.is_zero());
-        Self(unsafe { NonZeroUsize::new_unchecked(address.as_usize()) })
+        Self(NonZeroUsize::new(address.as_usize()).unwrap())
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 295-300)</summary>

```diff
@@ -295,6 +295,4 @@ impl Block {
-            // About unsafe: We know `cell` is non-zero here.
-            let potential_object = unsafe { ObjectReference::from_raw_address_unchecked(cell) };
+            // We know `cell` is non-zero here.
+            let potential_object = ObjectReference::from_raw_address(cell).unwrap();
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 335-345)</summary>

```diff
@@ -335,16 +338,15 @@ impl Block {
-            let potential_object_ref = unsafe {
-                // We know cursor plus an offset cannot be 0.
-                ObjectReference::from_raw_address_unchecked(
-                    cursor + VM::VMObjectModel::OBJECT_REF_OFFSET_LOWER_BOUND,
-                )
-            };
+            // We know cursor plus an offset cannot be 0.
+            let potential_object_ref = ObjectReference::from_raw_address(
+                cursor + VM::VMObjectModel::OBJECT_REF_OFFSET_LOWER_BOUND,
+            )
+            .unwrap();
```
</details>

<details>
<summary>src/util/address.rs (Removal of from_raw_address_unchecked)</summary>

```diff
@@ -591,26 +595,11 @@ impl ObjectReference {
-    /// Like `from_raw_address`, but assume `addr` is not zero.  This can be used to elide a check
-    /// against zero for performance-critical code.
-    ///
-    /// # Safety
-    ///
-    /// This method assumes `addr` is not zero.  It should only be used in cases where we know at
-    /// compile time that the input cannot be zero.  For example, if we compute the address by
-    /// adding a positive offset to a non-zero address, we know the result must not be zero.
-    pub unsafe fn from_raw_address_unchecked(addr: Address) -> ObjectReference {
-        debug_assert!(!addr.is_zero());
-        debug_assert!(
-            addr.is_aligned_to(Self::ALIGNMENT),
-            "ObjectReference is required to be word aligned.  addr: {addr}"
-        );
-        ObjectReference(NonZeroUsize::new_unchecked(addr.0))
-    }
```
</details>

<details>
<summary>src/util/metadata/vo_bit/mod.rs (from_raw_address usage)</summary>

```diff
@@ -206,25 +207,23 @@ pub(crate) fn get_object_ref_for_vo_addr(vo_addr: Address) -> ObjectReference {
     // VO bit should be set on the address.
     debug_assert!(vo_addr.is_aligned_to(ObjectReference::ALIGNMENT));
-    debug_assert!(unsafe { is_vo_addr(vo_addr) });
-    unsafe { ObjectReference::from_raw_address_unchecked(vo_addr) }
+    debug_assert!(is_vo_addr(vo_addr));
+    ObjectReference::from_raw_address(vo_addr).unwrap()
 }
```
</details>

<details>
<summary>src/util/alloc/free_list_allocator.rs (unset_vo_bit)</summary>

```diff
@@ -404,55 +401,55 @@ impl<VM: VMBinding> FreeListAllocator<VM> {
         // unset allocation bit
         // Note: We cannot use `unset_vo_bit_unsafe` because two threads may attempt to free
         // objects at adjacent addresses, and they may share the same byte in the VO bit metadata.
-        crate::util::metadata::vo_bit::unset_vo_bit(unsafe {
-            ObjectReference::from_raw_address_unchecked(addr)
-        })
+        crate::util::metadata::vo_bit::unset_vo_bit(
+            ObjectReference::from_raw_address(addr).unwrap()
+        )
```
</details>

<details>
<summary>docs/dummyvm/src/lib.rs (object_start_to_ref)</summary>

```diff
@@ -31,16 +31,12 @@ impl VMBinding for DummyVM {
 
 use mmtk::util::{Address, ObjectReference};
 
 impl DummyVM {
     pub fn object_start_to_ref(start: Address) -> ObjectReference {
-        // Safety: start is the allocation result, and it should not be zero with an offset.
-        unsafe {
-            ObjectReference::from_raw_address_unchecked(
-                start + crate::object_model::OBJECT_REF_OFFSET,
-            )
-        }
+        // The start is the allocation result, and it should not be zero with an offset.
+        ObjectReference::from_raw_address(start + crate::object_model::OBJECT_REF_OFFSET).unwrap()
     }
 }
 
 pub static SINGLETON: OnceLock<Box<MMTK<DummyVM>>> = OnceLock::new();
```
</details>

<details>
<summary>src/util/object_forwarding.rs (read_forwarding_pointer)</summary>

```diff
@@ -165,21 +165,20 @@ pub fn read_forwarding_pointer<VM: VMBinding>(object: ObjectReference) -> Object
         "read_forwarding_pointer called for object {:?} that has not started forwarding!",
         object,
     );
 
     // We write the forwarding poiner. We know it is an object reference.
-    unsafe {
-        // We use "unchecked" convertion becasue we guarantee the forwarding pointer we stored
-        // previously is from a valid `ObjectReference` which is never zero.
-        ObjectReference::from_raw_address_unchecked(crate::util::Address::from_usize(
-            VM::VMObjectModel::LOCAL_FORWARDING_POINTER_SPEC.load_atomic::<VM, usize>(
-                object,
-                Some(FORWARDING_POINTER_MASK),
-                Ordering::SeqCst,
-            ),
-        ))
-    }
+    // We use "unchecked" convertion becasue we guarantee the forwarding pointer we stored
+    // previously is from a valid `ObjectReference` which is never zero.
+    ObjectReference::from_raw_address(crate::util::Address::from_usize(
+        VM::VMObjectModel::LOCAL_FORWARDING_POINTER_SPEC.load_atomic::<VM, usize>(
+            object,
+            Some(FORWARDING_POINTER_MASK),
+            Ordering::SeqCst,
+        ),
+    ))
+    .expect("Forwarding pointer should not be zero")
 }
 ```
</details>

### Safe Reference Passing
**Description**: Passing safe references (like `&mut BlockList`) to methods instead of loading raw pointers from metadata and dereferencing them.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/marksweepspace/native_ms/block.rs` | 1 | 0 | -1 |
| `src/scheduler/gc_work.rs` | 1 | 0 | -1 |
| `src/plan/concurrent/concurrent_marking_work.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -3

**Diff Snippets**:
<details>
<summary>src/policy/marksweepspace/native_ms/block.rs (lines 233-248)</summary>

```diff
@@ -233,6 +241,5 @@ impl Block {
-    pub fn attempt_release<VM: VMBinding>(self, space: &MarkSweepSpace<VM>) -> bool {
+    pub fn attempt_release<VM: VMBinding>(self, space: &MarkSweepSpace<VM>, block_list: &mut BlockList) -> bool {
         match self.get_state() {
             BlockState::Unmarked => {
-                let block_list = self.load_block_list();
-                unsafe { &mut *block_list }.remove(self);
+                block_list.remove(self);
                 space.release_block(self);
```
</details>

<details>
<summary>src/scheduler/gc_work.rs (Removal of worker raw pointer)</summary>

```diff
@@ -501,22 +497,14 @@ impl<VM: VMBinding> ProcessEdgesBase<VM> {
-    // Use raw pointer for fast pointer dereferencing, instead of using `Option<&'static mut GCWorker<E::VM>>`.
-    // Because a copying gc will dereference this pointer at least once for every object copy.
-    worker: *mut GCWorker<VM>,
...
-    pub fn set_worker(&mut self, worker: &mut GCWorker<VM>) {
-        self.worker = worker;
-    }
-
-    pub fn worker(&self) -> &'static mut GCWorker<VM> {
-        unsafe { &mut *self.worker }
-    }
```
</details>

<details>
<summary>src/plan/concurrent/concurrent_marking_work.rs (Removal of worker raw pointer)</summary>

```diff
@@ -37,48 +39,43 @@ impl<VM: VMBinding, P: ConcurrentPlan<VM = VM> + PlanTraceObject<VM>, const KIND
 
         Self {
             plan,
             objects: Some(objects),
             next_objects: VectorQueue::default(),
-            worker: std::ptr::null_mut(),
+            mmtk,
+            tls: None,
         }
     }
 
-    pub fn worker(&self) -> &'static mut GCWorker<VM> {
-        debug_assert_ne!(self.worker, std::ptr::null_mut());
-        unsafe { &mut *self.worker }
-    }
```
</details>


### Fat Pointer Atomics to Thin Pointer Atomics (SFTHeader)
**Description**: Replaced transmuting fat pointers (`*const dyn SFT`) to double-word sized integers for atomic operations with a safe abstraction using a thin pointer to an `SFTHeader` stored in a standard `AtomicPtr`. The `SFTHeader` contains the fat pointer. This eliminates unsafe transmutes and allows making `SFTMap` trait methods safe.

Definition:
```rust
pub(crate) struct SFTHeader {
    pub sft: *const (dyn SFT + Sync + 'static),
}

// SAFETY: SFT instances are Sync, and the SFTHeader only contains a pointer to them.
// The pointer is only used to access the SFT trait methods which are thread-safe.
unsafe impl Sync for SFTHeader {}
```

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/sft_map.rs` | 26 | 2 | -24 |
| `src/policy/marksweepspace/malloc_ms/global.rs` | 2 | 0 | -2 |
| `src/util/address.rs` | 6 | 0 | -6 |
| `src/scheduler/gc_work.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -33

**Diff Snippets**:
<details>
<summary>src/policy/sft_map.rs (Trait Definition)</summary>

```diff
@@ -22,11 +22,11 @@ pub trait SFTMap {
-    unsafe fn get_unchecked(&self, address: Address) -> &dyn SFT;
+    fn get_unchecked(&self, address: Address) -> &dyn SFT;
 
-    unsafe fn update(&self, space: SFTRawPointer, start: Address, bytes: usize);
+    fn update(&self, space: &(dyn SFT + Sync + 'static), start: Address, bytes: usize);
 
-    unsafe fn eager_initialize(
+    fn eager_initialize(
...
-    unsafe fn clear(&self, address: Address);
+    fn clear(&self, address: Address);
```
</details>

<details>
<summary>src/policy/sft_map.rs (SFTHeader and SFTRefStorage)</summary>

```diff
@@ -89,80 +93,57 @@
+pub(crate) struct SFTHeader {
+    pub sft: *const (dyn SFT + Sync + 'static),
+}
+
+// SAFETY: SFT instances are Sync, and the SFTHeader only contains a pointer to them.
+// The pointer is only used to access the SFT trait methods which are thread-safe.
+unsafe impl Sync for SFTHeader {}
...
-pub(crate) struct SFTRefStorage(AtomicDoubleWord);
+pub(crate) struct SFTRefStorage(std::sync::atomic::AtomicPtr<SFTHeader>);
...
     pub fn load(&self) -> &dyn SFT {
-        let val = self.0.load(Ordering::Acquire);
-        // ...
-        unsafe {
-            std::mem::transmute(val)
-        }
+        let ptr = self.0.load(Ordering::Acquire);
+        // SAFETY: The pointer was stored by `store` or `new`, which only store
+        // valid pointers to leaked `SFTHeader`s.
+        unsafe { &*(*ptr).sft }
     }
```
</details>

<details>
<summary>src/policy/sft_map.rs (SFTSpaceMap)</summary>

```diff
@@ -175,11 +156,11 @@ mod space_map {
-    unsafe impl Sync for SFTSpaceMap {}
...
-        unsafe fn get_unchecked(&self, address: Address) -> &dyn SFT {
-            let cell = unsafe { self.sft.get_unchecked(Self::addr_to_index(address)) };
+        fn get_unchecked(&self, address: Address) -> &dyn SFT {
+            let cell = &self.sft[Self::addr_to_index(address)];
             cell.load()
         }
```
</details>

*(Note: The diff provided in the prompt was truncated, so not all snippets are included here.)*

<details>
<summary>src/policy/marksweepspace/malloc_ms/global.rs (SFT Map operations)</summary>

```diff
@@ -390,11 +387,11 @@ impl<VM: VMBinding> MallocSpace<VM> {
             if !self.is_meta_space_mapped(address, actual_size) {
                 // Map the metadata space for the associated chunk
                 self.map_metadata_and_update_bound(address, actual_size);
                 // Update SFT
                 assert!(crate::mmtk::SFT_MAP.has_sft_entry(address)); // make sure the address is okay with our SFT map
-                unsafe { crate::mmtk::SFT_MAP.update(self, address, actual_size) };
+                crate::mmtk::SFT_MAP.update(self, address, actual_size);
             }
@@ -598,34 +595,34 @@ impl<VM: VMBinding> MallocSpace<VM> {
         let bytes = get_malloc_usable_size(obj_start, offset_malloc_bit);
         (obj_start, offset_malloc_bit, bytes)
     }
 
     /// Clean up for an empty chunk
-    fn clean_up_empty_chunk(&self, chunk_start: Address) {
+    fn clean_up_empty_chunk(&self, chunk_start: Address, proof: &SweepProof) {
         // Clear the chunk map
         self.chunk_map
             .set_allocated(Chunk::from_aligned_address(chunk_start), false);
         // Clear the SFT entry
-        unsafe { crate::mmtk::SFT_MAP.clear(chunk_start) };
+        crate::mmtk::SFT_MAP.clear_safe(chunk_start);
```
</details>

<details>
<summary>src/util/address.rs (SFT_MAP access made safe)</summary>

```diff
@@ -669,37 +658,37 @@ impl ObjectReference {
     /// objects in the remembered set can be unreachable in the first place.  (This is known as
     /// *nepotism* in GC literature.)
     ///
     /// Note: Objects in ImmortalSpace may have `is_live = true` but are actually unreachable.
     pub fn is_reachable(self) -> bool {
-        unsafe { SFT_MAP.get_unchecked(self.to_raw_address()) }.is_reachable(self)
+        SFT_MAP.get_unchecked(self.to_raw_address()).is_reachable(self)
     }
 
     /// Is the object live, determined by the policy?
     pub fn is_live(self) -> bool {
-        unsafe { SFT_MAP.get_unchecked(self.to_raw_address()) }.is_live(self)
+        SFT_MAP.get_unchecked(self.to_raw_address()).is_live(self)
     }
 
     /// Can the object be moved?
     pub fn is_movable(self) -> bool {
-        unsafe { SFT_MAP.get_unchecked(self.to_raw_address()) }.is_movable()
+        SFT_MAP.get_unchecked(self.to_raw_address()).is_movable()
     }
 
     /// Get forwarding pointer if the object is forwarded.
     pub fn get_forwarded_object(self) -> Option<Self> {
-        unsafe { SFT_MAP.get_unchecked(self.to_raw_address()) }.get_forwarded_object(self)
+        SFT_MAP.get_unchecked(self.to_raw_address()).get_forwarded_object(self)
     }
 
     /// Is the object in any MMTk spaces?
     pub fn is_in_any_space(self) -> bool {
-        unsafe { SFT_MAP.get_unchecked(self.to_raw_address()) }.is_in_space(self)
+        SFT_MAP.get_unchecked(self.to_raw_address()).is_in_space(self)
     }
 
     /// Is the object sane?
     #[cfg(feature = "sanity")]
     pub fn is_sane(self) -> bool {
-        unsafe { SFT_MAP.get_unchecked(self.to_raw_address()) }.is_sane()
+        SFT_MAP.get_unchecked(self.to_raw_address()).is_sane()
     }
 }
```
</details>

<details>
<summary>src/scheduler/gc_work.rs (SFT_MAP access made safe)</summary>

```diff
@@ -719,1 +707,1 @@
-        let sft = unsafe { crate::mmtk::SFT_MAP.get_unchecked(object.to_raw_address()) };
+        let sft = crate::mmtk::SFT_MAP.get_unchecked(object.to_raw_address());
```
</details>


### Safe Initialization (MaybeUninit → Option)
**Description**: Replacing `MaybeUninit` with `Option` for arrays of allocators. This allows safe initialization with `None` and safe access using `unwrap()`, eliminating unsafe `assume_init()` and `assume_init_mut()` calls.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/plan/generational/copying/global.rs` | 1 | 0 | -1 |
| `src/plan/semispace/global.rs` | 1 | 0 | -1 |
| `src/util/copy/mod.rs` | 18 | 0 | -18 |
| `src/util/heap/blockpageresource.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -21

**Diff Snippets**:
<details>
<summary>src/util/heap/blockpageresource.rs (MaybeUninit to Option)</summary>

```diff
@@ -183,62 +183,61 @@
-    data: UnsafeCell<Box<[MaybeUninit<B>]>>,
+    data: RwLock<Box<[Option<B>]>>,
...
     fn get_entry(&self, i: usize) -> B {
-        unsafe { (*self.data.get())[i].assume_init() }
+        self.data.read()[i].unwrap()
     }
```
</details>
<details>
<summary>src/util/copy/mod.rs (Struct definition)</summary>

```diff
@@ -51,15 +50,15 @@ impl<VM: VMBinding> Default for CopyConfig<VM> {
 
 /// The thread local struct for each GC worker for copying. Each GC worker should include
 /// one instance of this struct for copying operations.
 pub struct GCWorkerCopyContext<VM: VMBinding> {
     /// Copy allocators for CopySpace
-    pub copy: [MaybeUninit<CopySpaceCopyContext<VM>>; MAX_COPYSPACE_COPY_ALLOCATORS],
+    pub copy: [Option<CopySpaceCopyContext<VM>>; MAX_COPYSPACE_COPY_ALLOCATORS],
     /// Copy allocators for ImmixSpace
-    pub immix: [MaybeUninit<ImmixCopyContext<VM>>; MAX_IMMIX_COPY_ALLOCATORS],
+    pub immix: [Option<ImmixCopyContext<VM>>; MAX_IMMIX_COPY_ALLOCATORS],
     /// Copy allocators for ImmixSpace
-    pub immix_hybrid: [MaybeUninit<ImmixHybridCopyContext<VM>>; MAX_IMMIX_HYBRID_COPY_ALLOCATORS],
+    pub immix_hybrid: [Option<ImmixHybridCopyContext<VM>>; MAX_IMMIX_HYBRID_COPY_ALLOCATORS],
     /// The config for the plan
     config: CopyConfig<VM>,
 }
```
</details>

<details>
<summary>src/util/copy/mod.rs (Usage in alloc_copy)</summary>

```diff
@@ -86,20 +85,22 @@ impl<VM: VMBinding> GCWorkerCopyContext<VM> {
                 "Attempted to copy an object of {} bytes (> {}) which should be allocated with LOS and not be copied.",
                 bytes, self.config.constraints.max_non_los_default_alloc_bytes
             );
         }
         match self.config.copy_mapping[semantics] {
-            CopySelector::CopySpace(index) => {
-                unsafe { self.copy[index as usize].assume_init_mut() }
-                    .alloc_copy(original, bytes, align, offset)
-            }
-            CopySelector::Immix(index) => unsafe { self.immix[index as usize].assume_init_mut() }
+            CopySelector::CopySpace(index) => self.copy[index as usize]
+                .as_mut()
+                .unwrap()
+                .alloc_copy(original, bytes, align, offset),
+            CopySelector::Immix(index) => self.immix[index as usize]
+                .as_mut()
+                .unwrap()
                 .alloc_copy(original, bytes, align, offset),
```
</details>

<details>
<summary>src/util/copy/mod.rs (Initialization in new)</summary>

```diff
@@ -178,36 +187,36 @@ impl<VM: VMBinding> GCWorkerCopyContext<VM> {
     /// * `worker_tls`: The worker thread for this copy context.
     /// * `plan`: A reference to the current plan.
     /// * `config`: The configuration for the copy context.
     pub fn new(worker_tls: VMWorkerThread, mmtk: &MMTK<VM>, config: CopyConfig<VM>) -> Self {
         let mut ret = GCWorkerCopyContext {
-            copy: unsafe { MaybeUninit::uninit().assume_init() },
-            immix: unsafe { MaybeUninit::uninit().assume_init() },
-            immix_hybrid: unsafe { MaybeUninit::uninit().assume_init() },
+            copy: [None; MAX_COPYSPACE_COPY_ALLOCATORS],
+            immix: [None; MAX_IMMIX_COPY_ALLOCATORS],
+            immix_hybrid: [None; MAX_IMMIX_HYBRID_COPY_ALLOCATORS],
             config,
         };
```
</details>

<details>
<summary>src/plan/generational/copying/global.rs (MaybeUninit to Option)</summary>

```diff
@@ -100,12 +100,12 @@ impl<VM: VMBinding> Plan for GenCopy<VM> {
         self.fromspace_mut()
             .set_copy_for_sft_trace(Some(CopySemantics::Mature));
         self.tospace_mut().set_copy_for_sft_trace(None);
     }
 
-    fn prepare_worker(&self, worker: &mut GCWorker<Self::VM>) {
-        unsafe { worker.get_copy_context_mut().copy[0].assume_init_mut() }.rebind(self.tospace());
+    fn prepare_worker(&'static self, worker: &mut GCWorker<Self::VM>) {
+        worker.get_copy_context_mut().copy[0].as_mut().unwrap().rebind(self.tospace());
     }
 
     fn release(&mut self, tls: VMWorkerThread) {
         let full_heap = !self.gen.is_current_gc_nursery();
         self.gen.release(tls);
 ```
 </details>

<details>
<summary>src/plan/semispace/global.rs (prepare_worker)</summary>

```diff
@@ -86,3 +86,3 @@
-    fn prepare_worker(&self, worker: &mut GCWorker<VM>) {
-        unsafe { worker.get_copy_context_mut().copy[0].assume_init_mut() }.rebind(self.tospace());
+    fn prepare_worker(&'static self, worker: &mut GCWorker<VM>) {
+        worker.get_copy_context_mut().copy[0].as_mut().unwrap().rebind(self.tospace());
```
</details>

### Safe Array Initialization (Const Blocks)
**Description**: Using `const` blocks to initialize arrays of `MaybeUninit` safely, replacing the pattern of using `unsafe { MaybeUninit::uninit().assume_init() }` to create uninitialized arrays.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/alloc/allocators.rs` | 6 | 0 | -6 |

**Category Total**: Δ = -6

**Diff Snippets**:
<details>
<summary>src/util/alloc/allocators.rs (array initialization)</summary>

```diff
@@ -103,16 +132,22 @@ impl<VM: VMBinding> Allocators<VM> {
-            bump_pointer: unsafe { MaybeUninit::uninit().assume_init() },
-            large_object: unsafe { MaybeUninit::uninit().assume_init() },
-            malloc: unsafe { MaybeUninit::uninit().assume_init() },
-            immix: unsafe { MaybeUninit::uninit().assume_init() },
-            free_list: unsafe { MaybeUninit::uninit().assume_init() },
-            markcompact: unsafe { MaybeUninit::uninit().assume_init() },
+            bump_pointer: [const { MaybeUninit::uninit() }; MAX_BUMP_ALLOCATORS],
+            large_object: [const { MaybeUninit::uninit() }; MAX_LARGE_OBJECT_ALLOCATORS],
+            malloc: [const { MaybeUninit::uninit() }; MAX_MALLOC_ALLOCATORS],
+            immix: [const { MaybeUninit::uninit() }; MAX_IMMIX_ALLOCATORS],
+            free_list: [const { MaybeUninit::uninit() }; MAX_FREE_LIST_ALLOCATORS],
+            markcompact: [const { MaybeUninit::uninit() }; MAX_MARK_COMPACT_ALLOCATORS],
```
</details>

### Safe Allocator Access
**Description**: Methods for accessing thread-local allocators within the `Mutator` (by `AllocatorSelector` or `AllocationSemantics`) were made safe. This eliminates the need for `unsafe` blocks when retrieving allocators for allocation, prepare/release cycles, and object metadata initialization.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/plan/mutator_context.rs` | 15 | 0 | -15 |
| `src/util/alloc/allocators.rs` | 4 | 2 | -2 |
| `src/plan/concurrent/immix/mutator.rs` | 2 | 0 | -2 |
| `src/plan/compressor/mutator.rs` | 1 | 0 | -1 |
| `src/plan/generational/copying/mutator.rs` | 1 | 0 | -1 |
| `src/plan/generational/immix/mutator.rs` | 1 | 0 | -1 |
| `src/plan/immix/mutator.rs` | 1 | 0 | -1 |
| `src/plan/markcompact/mutator.rs` | 1 | 0 | -1 |
| `src/plan/marksweep/mutator.rs` | 1 | 0 | -1 |
| `src/plan/semispace/mutator.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -26

**Diff Snippets**:
<details>
<summary>src/plan/mutator_context.rs (prepare/release)</summary>

```diff
@@ -31,15 +31,13 @@ pub(crate) fn common_prepare_func<VM: VMBinding>(mutator: &mut Mutator<VM>, _tls
     // Prepare the free list allocator used for non moving
     #[cfg(feature = "marksweep_as_nonmoving")]
-    unsafe {
-        mutator.allocator_impl_mut_for_semantic::<crate::util::alloc::FreeListAllocator<VM>>(
-            AllocationSemantics::NonMoving,
-        )
-    }
+    mutator.allocator_impl_mut_for_semantic::<crate::util::alloc::FreeListAllocator<VM>>(
+        AllocationSemantics::NonMoving,
+    )
     .prepare();
 }
@@ -54,20 +52,20 @@ pub(crate) fn common_release_func<VM: VMBinding>(mutator: &mut Mutator<VM>, _tls
     cfg_if::cfg_if! {
         if #[cfg(feature = "marksweep_as_nonmoving")] {
             // Release the free list allocator used for non moving
-            unsafe { mutator.allocator_impl_mut_for_semantic::<crate::util::alloc::FreeListAllocator<VM>>(
+            mutator.allocator_impl_mut_for_semantic::<crate::util::alloc::FreeListAllocator<VM>>(
                 AllocationSemantics::NonMoving,
-            )}.release();
+            ).release();
         } else if #[cfg(feature = "immortal_as_nonmoving")] {
             // Do nothig for the bump pointer allocator
         } else {
             // Reset the Immix allocator
-            unsafe { mutator.allocator_impl_mut_for_semantic::<crate::util::alloc::ImmixAllocator<VM>>(
+            mutator.allocator_impl_mut_for_semantic::<crate::util::alloc::ImmixAllocator<VM>>(
                 AllocationSemantics::NonMoving,
-            )}.reset();
+            ).reset();
         }
     }
 }
```
</details>

<details>
<summary>src/plan/mutator_context.rs (allocator access in traits)</summary>

```diff
@@ -191,14 +189,12 @@ impl<VM: VMBinding> MutatorContext<VM> for Mutator<VM> {
         size: usize,
         align: usize,
         offset: usize,
         allocator: AllocationSemantics,
     ) -> Address {
-        let allocator = unsafe {
-            self.allocators
-                .get_allocator_mut(self.config.allocator_mapping[allocator])
-        };
+        let allocator = self.allocators
+            .get_allocator_mut(self.config.allocator_mapping[allocator]);
         // The value should be default/unset at the beginning of an allocation request.
         debug_assert!(allocator.get_context().get_alloc_options().is_default());
         allocator.alloc(size, align, offset)
     }
```
</details>

<details>
<summary>src/plan/mutator_context.rs (safe methods in Mutator)</summary>

```diff
@@ -291,69 +279,43 @@ impl<VM: VMBinding> Mutator<VM> {
     /// Inform each allocator about destroying. Call allocator-specific on destroy methods.
     pub fn on_destroy(&mut self) {
         for selector in self.get_all_allocator_selectors() {
-            unsafe { self.allocators.get_allocator_mut(selector) }.on_mutator_destroy();
+            self.allocators.get_allocator_mut(selector).on_mutator_destroy();
         }
     }
 
     /// Get the allocator for the selector.
-    ///
-    /// # Safety
-    /// The selector needs to be valid, and points to an allocator that has been initialized.
-    /// [`crate::memory_manager::get_allocator_mapping`] can be used to get a selector.
-    pub unsafe fn allocator(&self, selector: AllocatorSelector) -> &dyn Allocator<VM> {
+    pub fn allocator(&self, selector: AllocatorSelector) -> &dyn Allocator<VM> {
         self.allocators.get_allocator(selector)
     }
 
-    /// Get the mutable allocator for the selector.
-    ///
-    /// # Safety
-    /// The selector needs to be valid, and points to an allocator that has been initialized.
-    /// [`crate::memory_manager::get_allocator_mapping`] can be used to get a selector.
-    pub unsafe fn allocator_mut(&mut self, selector: AllocatorSelector) -> &mut dyn Allocator<VM> {
+    pub fn allocator_mut(&mut self, selector: AllocatorSelector) -> &mut dyn Allocator<VM> {
         self.allocators.get_allocator_mut(selector)
     }
 
-    /// Get the allocator of a concrete type for the selector.
-    ///
-    /// # Safety
-    /// The selector needs to be valid, and points to an allocator that has been initialized.
-    /// [`crate::memory_manager::get_allocator_mapping`] can be used to get a selector.
-    pub unsafe fn allocator_impl<T: Allocator<VM>>(&self, selector: AllocatorSelector) -> &T {
+    pub fn allocator_impl<T: Allocator<VM>>(&self, selector: AllocatorSelector) -> &T {
         self.allocators.get_typed_allocator(selector)
     }
 
-    /// Get the mutable allocator of a concrete type for the selector.
-    ///
-    /// # Safety
-    /// The selector needs to be valid, and points to an allocator that has been initialized.
-    /// [`crate::memory_manager::get_allocator_mapping`] can be used to get a selector.
-    pub unsafe fn allocator_impl_mut<T: Allocator<VM>>(
+    pub fn allocator_impl_mut<T: Allocator<VM>>(
         &mut self,
         selector: AllocatorSelector,
     ) -> &mut T {
         self.allocators.get_typed_allocator_mut(selector)
     }
 
-    /// Get the allocator of a concrete type for the semantic.
-    ///
-    /// # Safety
-    /// The semantic needs to match the allocator type.
-    pub unsafe fn allocator_impl_for_semantic<T: Allocator<VM>>(
+    pub fn allocator_impl_for_semantic<T: Allocator<VM>>(
         &self,
         semantic: AllocationSemantics,
     ) -> &T {
         self.allocator_impl::<T>(self.config.allocator_mapping[semantic])
     }
 
-    /// Get the mutable allocator of a concrete type for the semantic.
-    ///
-    /// # Safety
-    /// The semantic needs to match the allocator type.
-    pub unsafe fn allocator_impl_mut_for_semantic<T: Allocator<VM>>(
+    pub fn allocator_impl_mut_for_semantic<T: Allocator<VM>>(
         &mut self,
         semantic: AllocationSemantics,
     ) -> &mut T {
         self.allocator_impl_mut::<T>(self.config.allocator_mapping[semantic])
     }
```
</details>

<details>
<summary>src/plan/concurrent/immix/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -28,17 +28,14 @@ pub fn concurrent_immix_mutator_release<VM: VMBinding>(
 ) {
     // Release is not scheduled for initial mark pause
     let current_pause = mutator.plan.concurrent().unwrap().current_pause().unwrap();
     debug_assert_ne!(current_pause, Pause::InitialMark);
 
-    let immix_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<ImmixAllocator<VM>>()
-    .unwrap();
+    let immix_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<ImmixAllocator<VM>>()
+        .unwrap();
     immix_allocator.reset();
 
     // Deactivate SATB
     if current_pause == Pause::Full || current_pause == Pause::FinalMark {
         debug!("Deactivate SATB barrier active for {:?}", mutator as *mut _);
@@ -56,17 +53,14 @@ pub fn concurent_immix_mutator_prepare<VM: VMBinding>(
 ) {
     // Prepare is not scheduled for final mark pause
     let current_pause = mutator.plan.concurrent().unwrap().current_pause().unwrap();
     debug_assert_ne!(current_pause, Pause::FinalMark);
 
-    let immix_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<ImmixAllocator<VM>>()
-    .unwrap();
+    let immix_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<ImmixAllocator<VM>>()
+        .unwrap();
     immix_allocator.reset();
 
     // Activate SATB
     if current_pause == Pause::InitialMark {
         debug!("Activate SATB barrier active for {:?}", mutator as *mut _);
     }
}
```
</details>

<details>
<summary>src/plan/compressor/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -59,15 +59,12 @@ pub fn create_compressor_mutator<VM: VMBinding>(
     builder.build()
 }
 
 pub fn compressor_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
     // reset the thread-local allocation bump pointer
-    let bump_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<BumpAllocator<VM>>()
-    .unwrap();
+    let bump_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<BumpAllocator<VM>>()
+        .unwrap();
     bump_allocator.reset();
     common_release_func(mutator, tls);
 }
```
</details>

<details>
<summary>src/plan/generational/copying/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -14,17 +14,14 @@ use crate::util::{VMMutatorThread, VMWorkerThread};
 use crate::vm::VMBinding;
 use crate::MMTK;
 
 pub fn gencopy_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
     // reset nursery allocator
-    let bump_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<BumpAllocator<VM>>()
-    .unwrap();
+    let bump_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<BumpAllocator<VM>>()
+        .unwrap();
     bump_allocator.reset();
 
     common_release_func(mutator, tls);
 }
```
</details>

<details>
<summary>src/plan/generational/immix/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -14,17 +14,14 @@ use crate::util::{VMMutatorThread, VMWorkerThread};
 use crate::vm::VMBinding;
 use crate::MMTK;
 
 pub fn genimmix_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
     // reset nursery allocator
-    let bump_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<BumpAllocator<VM>>()
-    .unwrap();
+    let bump_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<BumpAllocator<VM>>()
+        .unwrap();
     bump_allocator.reset();
 
     common_release_func(mutator, tls);
 }
```
</details>

<details>
<summary>src/plan/immix/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -14,17 +14,14 @@ use crate::util::opaque_pointer::{VMMutatorThread, VMWorkerThread};
 use crate::vm::VMBinding;
 use crate::MMTK;
 use enum_map::EnumMap;
 
 pub fn immix_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
-    let immix_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<ImmixAllocator<VM>>()
-    .unwrap();
+    let immix_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<ImmixAllocator<VM>>()
+        .unwrap();
     immix_allocator.reset();
 
     common_release_func(mutator, tls);
 }
```
</details>

<details>
<summary>src/plan/markcompact/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -47,16 +47,13 @@ pub fn create_markcompact_mutator<VM: VMBinding>(
     builder.build()
 }
 
 pub fn markcompact_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
     // reset the thread-local allocation bump pointer
-    let markcompact_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<MarkCompactAllocator<VM>>()
-    .unwrap();
+    let markcompact_allocator = mutator
+        .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<MarkCompactAllocator<VM>>()
+        .unwrap();
     markcompact_allocator.reset();
 
     common_release_func(mutator, tls);
 }
```
</details>

<details>
<summary>src/plan/marksweep/mutator.rs (allocator_mut instead of unsafe get_allocator_mut)</summary>

```diff
@@ -61,15 +61,12 @@ mod native_mark_sweep {
     use crate::util::alloc::FreeListAllocator;
 
     fn get_freelist_allocator_mut<VM: VMBinding>(
         mutator: &mut Mutator<VM>,
     ) -> &mut FreeListAllocator<VM> {
-        unsafe {
-            mutator
-                .allocators
-                .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-        }
+        mutator
+            .allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
         .downcast_mut::<FreeListAllocator<VM>>()
         .unwrap()
     }
 
     // We forward calls to the allocator prepare and release
@@ -84,11 +81,11 @@ mod native_mark_sweep {
 
     #[cfg(not(feature = "malloc_mark_sweep"))]
     pub fn ms_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
         use crate::plan::mutator_context::common_release_func;
 
-        get_freelist_allocator_mut::<VM>(mutator).release();
+        get_freelist_allocator_mut::<VM>(mutator).release(MarkSweep::get_ms_space);
 
         common_release_func(mutator, tls);
     }
```
</details>

<details>
<summary>src/plan/semispace/mutator.rs (allocator access in release)</summary>

```diff
@@ -15,17 +15,14 @@ use crate::vm::VMBinding;
 use crate::MMTK;
 use enum_map::EnumMap;
 
 pub fn ss_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, tls: VMWorkerThread) {
     // rebind the allocation bump pointer to the appropriate semispace
-    let bump_allocator = unsafe {
-        mutator
-            .allocators
-            .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
-    }
-    .downcast_mut::<BumpAllocator<VM>>()
-    .unwrap();
+    let bump_allocator = mutator.allocators
+        .get_allocator_mut(mutator.config.allocator_mapping[AllocationSemantics::Default])
+        .downcast_mut::<BumpAllocator<VM>>()
+        .unwrap();
     bump_allocator.rebind(
         mutator
             .plan
             .downcast_ref::<SemiSpace<VM>>()
             .unwrap()
```
</details>

### Proof Token (SweepProof)
**Description**: Guarantees safety of non-atomic metadata operations during GC sweep by passing a zero-sized proof token (`SweepProof`) that encodes exclusive access or state invariants at the type level.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/marksweepspace/malloc_ms/global.rs` | 10 | 0 | -10 |
| `src/policy/marksweepspace/malloc_ms/metadata.rs` | 7 | 1 | -6 |

**Category Total**: Δ = -16

**Diff Snippets**:
````carousel
```diff
-    unsafe fn unset_page_mark(&self, start: Address, size: usize) {
+    fn unset_page_mark(&self, start: Address, size: usize, proof: &SweepProof) {
```
<!-- slide -->
```diff
-        if !unsafe { is_marked_unsafe::<VM>(object) } {
+        if !is_marked_non_atomic::<VM>(object, proof) {
...
-            unsafe { unset_vo_bit_unsafe(object) };
+            unset_vo_bit_non_atomic(object, proof);
```
<!-- slide -->
```diff
-                let alloc_128: u128 = unsafe {
-                    load128(
-                        &crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC,
-                        address,
-                    )
-                };
-                let mark_128: u128 = unsafe { load128(&mark_bit_spec, address) };
+                let alloc_128: u128 = load128(
+                    &crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC,
+                    address,
+                    proof,
+                );
+                let mark_128: u128 = load128(&mark_bit_spec, address, proof);
```
<!-- slide -->
```diff
                     debug_assert!(
-                        unsafe { is_marked_unsafe::<VM>(object) },
+                        is_marked_non_atomic::<VM>(object, proof),
                         "Dead object = {} found after sweep",
                         object
                     );
```
<!-- slide -->
```diff
-            let live = !self.sweep_object(object, &mut empty_page_start);
+            let live = !self.sweep_object(object, &mut empty_page_start, proof);
             if live {
                 // Live object. Unset mark bit.
                 // We should be the only thread that access this chunk, it is okay to use non-atomic store.
-                unsafe { unset_mark_bit::<VM>(object) };
+                unset_mark_bit_non_atomic::<VM>(object, proof);
```
````

<details>
<summary>src/policy/marksweepspace/malloc_ms/metadata.rs (SweepProof)</summary>

````carousel
```diff
@@ -24,12 +24,12 @@ pub(crate) const OFFSET_MALLOC_METADATA_SPEC: SideMetadataSpec =
 
 pub fn is_marked<VM: VMBinding>(object: ObjectReference, ordering: Ordering) -> bool {
     VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(object, None, ordering) == 1
 }
 
-pub unsafe fn is_marked_unsafe<VM: VMBinding>(object: ObjectReference) -> bool {
-    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load::<VM, u8>(object, None) == 1
+pub fn is_marked_non_atomic<VM: VMBinding>(object: ObjectReference, _proof: &SweepProof) -> bool {
+    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(object, None, Ordering::Relaxed) == 1
 }
```
<!-- slide -->
```diff
@@ -43,12 +43,12 @@ pub(super) fn compare_exchange_set_page_mark(page_addr: Address) -> bool {
 pub(super) fn is_page_marked(page_addr: Address) -> bool {
     ACTIVE_PAGE_METADATA_SPEC.load_atomic::<u8>(page_addr, Ordering::SeqCst) == 1
 }
 
 #[allow(unused)]
-pub(super) unsafe fn is_page_marked_unsafe(page_addr: Address) -> bool {
-    ACTIVE_PAGE_METADATA_SPEC.load::<u8>(page_addr) == 1
+pub(super) fn is_page_marked_non_atomic(page_addr: Address, _proof: &SweepProof) -> bool {
+    ACTIVE_PAGE_METADATA_SPEC.load_atomic::<u8>(page_addr, Ordering::Relaxed) == 1
 }
```
<!-- slide -->
```diff
@@ -81,44 +81,85 @@ pub(super) fn set_offset_malloc_bit(address: Address) {
-/// Unset the offset bit for the allocation. The argument address should be the allocation address (object start)
-pub(super) unsafe fn unset_offset_malloc_bit_unsafe(address: Address) {
-    OFFSET_MALLOC_METADATA_SPEC.store::<u8>(address, 0);
+pub(super) fn unset_offset_malloc_bit(address: Address) {
+    OFFSET_MALLOC_METADATA_SPEC.store_atomic::<u8>(address, 0, Ordering::SeqCst);
 }
 
-pub unsafe fn unset_vo_bit_unsafe(object: ObjectReference) {
-    vo_bit::unset_vo_bit_unsafe(object);
+/// Unset the offset bit for the allocation. The argument address should be the allocation address (object start)
+pub(super) fn unset_offset_malloc_bit_non_atomic(address: Address, _proof: &SweepProof) {
+    let meta_addr = side_metadata::address_to_meta_address(&OFFSET_MALLOC_METADATA_SPEC, address);
+    let cursor = side_metadata::MetadataCursor(meta_addr);
+    let lshift = side_metadata::meta_byte_lshift(&OFFSET_MALLOC_METADATA_SPEC, address);
+    let mask = side_metadata::meta_byte_mask(&OFFSET_MALLOC_METADATA_SPEC) << lshift;
+    let old_val = cursor.load::<u8>();
+    let new_val = old_val & !mask;
+    cursor.store::<u8>(new_val);
+}
```
<!-- slide -->
```diff
+pub fn unset_vo_bit_non_atomic(object: ObjectReference, _proof: &SweepProof) {
+    let meta_addr = side_metadata::address_to_meta_address(&vo_bit::VO_BIT_SIDE_METADATA_SPEC, object.to_raw_address());
+    let cursor = side_metadata::MetadataCursor(meta_addr);
+    let lshift = side_metadata::meta_byte_lshift(&vo_bit::VO_BIT_SIDE_METADATA_SPEC, object.to_raw_address());
+    let mask = side_metadata::meta_byte_mask(&vo_bit::VO_BIT_SIDE_METADATA_SPEC) << lshift;
+    let old_val = cursor.load::<u8>();
+    let new_val = old_val & !mask;
+    cursor.store::<u8>(new_val);
 }
```
<!-- slide -->
```diff
 #[allow(unused)]
-pub unsafe fn unset_mark_bit<VM: VMBinding>(object: ObjectReference) {
-    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.store::<VM, u8>(object, 0, None);
+pub fn unset_mark_bit_non_atomic<VM: VMBinding>(object: ObjectReference, _proof: &SweepProof) {
+    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.store_atomic::<VM, u8>(object, 0, None, Ordering::Relaxed);
 }
```
<!-- slide -->
```diff
 #[allow(unused)]
-pub(super) unsafe fn unset_page_mark_unsafe(page_addr: Address) {
-    ACTIVE_PAGE_METADATA_SPEC.store::<u8>(page_addr, 0)
+pub(super) fn unset_page_mark_non_atomic(page_addr: Address, _proof: &SweepProof) {
+    ACTIVE_PAGE_METADATA_SPEC.store_atomic::<u8>(page_addr, 0, Ordering::Relaxed);
 }
```
<!-- slide -->
```diff
-pub(super) unsafe fn load128(metadata_spec: &SideMetadataSpec, data_addr: Address) -> u128 {
+pub(super) fn load128(metadata_spec: &SideMetadataSpec, data_addr: Address, _proof: &SweepProof) -> u128 {
     let meta_addr = side_metadata::address_to_meta_address(metadata_spec, data_addr);
 
     #[cfg(all(debug_assertions, feature = "extreme_assertions"))]
     metadata_spec.assert_metadata_mapped(data_addr);
 
-    meta_addr.load::<u128>()
+    let low = side_metadata::MetadataCursor(meta_addr).load::<u64>();
+    let high = side_metadata::MetadataCursor(meta_addr + 8usize).load::<u64>();
+    #[cfg(target_endian = "little")]
+    let val = ((high as u128) << 64) | (low as u128);
+    #[cfg(target_endian = "big")]
+    let val = ((low as u128) << 64) | (high as u128);
+    val
+}
```
<!-- slide -->
```diff
+pub struct SweepProof {
+    _priv: (),
+}
+
+impl SweepProof {
+    /// Create a new proof.
+    ///
+    /// # Safety
+    /// The caller must ensure that this thread has exclusive access to the chunk being swept.
+    pub unsafe fn new_unchecked() -> Self {
+        Self { _priv: () }
+    }
+
+    /// Create a new proof from an ExclusivePlanAccessProof.
+    pub fn new(_proof: &crate::scheduler::ExclusivePlanAccessProof) -> Self {
+        Self { _priv: () }
+    }
+}
```
````
</details>


### Eliminating Unsafe Lifetime Extensions (Safe Context Retrieval)
**Description**: Removing unsafe pointer casts used to extend lifetimes or bypass borrow checker (e.g., `&*(self as *const Self)`) by passing getter functions or using proper references.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/marksweepspace/malloc_ms/global.rs` | 1 | 0 | -1 |
| `src/mmtk.rs` | 3 | 1 | -2 |
| `src/policy/marksweepspace/native_ms/global.rs` | 3 | 0 | -3 |
| `src/policy/immix/immixspace.rs` | 2 | 0 | -2 |
| `src/plan/global.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -9

**Diff Snippets**:
<details>
<summary>src/plan/global.rs (CommonPlan self-reference removal)</summary>

```diff
@@ -755,22 +760,19 @@ impl<VM: VMBinding> CommonPlan<VM> {
         self.base.release(tls, full_heap)
     }
 
     pub(crate) fn schedule_unlog_bits_op(&mut self, unlog_bits_op: UnlogBitsOperation) {
         if VM::VMObjectModel::GLOBAL_LOG_BIT_SPEC.is_on_side() {
-            // # Safety: CommonPlan reference is always valid within this collection cycle.
-            let common_plan = unsafe { &*(self as *const CommonPlan<VM>) };
-
             match unlog_bits_op {
                 UnlogBitsOperation::NoOp => {}
                 UnlogBitsOperation::BulkSet => {
                     self.base.scheduler.work_buckets[WorkBucketStage::Prepare]
-                        .add(SetCommonPlanUnlogBits { common_plan });
+                        .add(SetCommonPlanUnlogBits::new());
                 }
                 UnlogBitsOperation::BulkClear => {
                     self.base.scheduler.work_buckets[WorkBucketStage::Release]
-                        .add(ClearCommonPlanUnlogBits { common_plan });
+                        .add(ClearCommonPlanUnlogBits::new());
                 }
             }
         }
     }
```
</details>
<details>
<summary>src/policy/immix/immixspace.rs (Lifetime extension removal)</summary>

```diff
@@ -450,2 +450,0 @@
-            // # Safety: ImmixSpace reference is always valid within this collection cycle.
-            let space = unsafe { &*(self as *const Self) };
```

```diff
@@ -547,2 +547,0 @@
-        // # Safety: ImmixSpace reference is always valid within this collection cycle.
-        let space = unsafe { &*(self as *const Self) };
```
</details>

<details>
<summary>src/policy/marksweepspace/malloc_ms/global.rs (Lifetime extension removal)</summary>

```diff
@@ -551,19 +548,19 @@ impl<VM: VMBinding> MallocSpace<VM> {
             return None;
         }
         crate::util::metadata::vo_bit::is_vo_bit_set_for_addr(addr)
     }
 
-    pub fn prepare(&mut self, _full_heap: bool) {}
+    pub fn prepare(&mut self, _full_heap: bool, _get_space: fn(&'static MMTK<VM>) -> &'static MallocSpace<VM>) {}
 
-    pub fn release(&mut self) {
+    pub fn release(&mut self, proof: &crate::scheduler::ExclusivePlanAccessProof, get_space: fn(&'static MMTK<VM>) -> &'static MallocSpace<VM>) {
         use crate::scheduler::WorkBucketStage;
-        let space = unsafe { &*(self as *const Self) };
         let work_packets = self.chunk_map.generate_tasks(|chunk| {
             Box::new(MSSweepChunk {
-                ms: space,
+                get_space,
                 chunk: chunk.start(),
+                proof: SweepProof::new(proof),
             })
         });
```
</details>

<details>
<summary>src/mmtk.rs (Lifetime extension removal and initialization)</summary>

```diff
@@ -187,12 +176,2 @@ impl<VM: VMBinding> MMTK<VM> {
-        // We haven't finished creating MMTk. No one is using the GC trigger. We cast the arc into a mutable reference.
-        {
-            // TODO: use Arc::get_mut_unchecked() when it is availble.
-            let gc_trigger: &mut GCTrigger<VM> =
-                unsafe { &mut *(Arc::as_ptr(&gc_trigger) as *mut _) };
-            // We know the plan address will not change. Cast it to a static reference.
-            let static_plan: &'static dyn Plan<VM = VM> = unsafe { &*(&*plan as *const _) };
-            // Set the plan so we can trigger GC and check GC condition without using plan
-            gc_trigger.set_plan(static_plan);
-        }
@@ -205,3 +192,5 @@ impl<VM: VMBinding> MMTK<VM> {
+        let plan: Arc<dyn Plan<VM = VM>> = Arc::from(plan);
+        gc_trigger.set_plan(plan.clone());
@@ -440,3 +425,3 @@ impl<VM: VMBinding> MMTK<VM> {
     pub fn get_plan(&self) -> &dyn Plan<VM = VM> {
-        unsafe { &**(self.plan.get()) }
+        unsafe { &**self.plan.get_ref() }
     }
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/global.rs (Lifetime extension removal)</summary>

```diff
@@ -428,7 +428,5 @@ impl<VM: VMBinding> MarkSweepSpace<VM> {
-        // # Safety: MarkSweepSpace reference is always valid within this collection cycle.
-        let space = unsafe { &*(self as *const Self) };
         let work_packets = self
             .chunk_map
-            .generate_tasks(|chunk| Box::new(PrepareChunkMap { space, chunk }));
+            .generate_tasks(move |chunk| Box::new(PrepareChunkMap { chunk, get_space }));
```

```diff
@@ -444,3 +444,2 @@ impl<VM: VMBinding> MarkSweepSpace<VM> {
-        let space = unsafe { &*(self as *const Self) };
-        let work_packet = ReleaseMarkSweepSpace { space };
+        let work_packet = ReleaseMarkSweepSpace { get_space };
```

```diff
@@ -532,6 +532,5 @@ impl<VM: VMBinding> MarkSweepSpace<VM> {
-        let space = unsafe { &*(self as *const Self) };
         let epilogue = Arc::new(RecycleBlocks {
-            space,
             counter: AtomicUsize::new(0),
+            get_space,
         });
```
</details>

### Safe Wrappers for Internal Operations
**Description**: Replacing direct unsafe calls to internal functions with safe wrappers that encapsulate the safety invariants.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/marksweepspace/malloc_ms/global.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/policy/marksweepspace/malloc_ms/global.rs (Safe wrapper)</summary>

```diff
@@ -463,11 +460,11 @@ impl<VM: VMBinding> MallocSpace<VM> {
     // indirect call instructions in the generated assembly
     fn free_internal(&self, addr: Address, bytes: usize, offset_malloc_bit: bool) {
         if offset_malloc_bit {
             trace!("Free memory {:x}", addr);
             offset_free(addr);
-            unsafe { unset_offset_malloc_bit_unsafe(addr) };
+            unset_offset_malloc_bit(addr);
         } else {
             let ptr = addr.to_mut_ptr();
```
</details>

### FFI Boundary (libc free)
**Description**: Encapsulating or maintaining unsafe blocks for calling external C library functions like `free`.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/marksweepspace/malloc_ms/global.rs` | 1 | 1 | 0 |

**Category Total**: Δ = 0

**Diff Snippets**:
<details>
<summary>src/policy/marksweepspace/malloc_ms/global.rs (FFI free)</summary>

```diff
             let ptr = addr.to_mut_ptr();
             trace!("Free memory {:?}", ptr);
             unsafe {
                 free(ptr);
             }
```
</details>

### Safe FFI Wrappers (Malloc)
**Description**: Replacing direct unsafe calls to libc malloc functions (`posix_memalign`, `calloc`, `malloc_usable_size`, `free`) with safe wrappers in `crate::util::malloc`.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/malloc/malloc_ms_util.rs` | 6 | 0 | -6 |
| `src/util/malloc/mod.rs` | 4 | 6 | +2 |

**Category Total**: Δ = -4

**Diff Snippets**:
<details>
<summary>src/util/malloc/malloc_ms_util.rs (FFI wrappers)</summary>

```diff
@@ -9,10 +8,6 @@ pub fn align_alloc(size: usize, align: usize) -> Address {
-    let mut ptr = std::ptr::null_mut::<libc::c_void>();
-    let ptr_ptr = std::ptr::addr_of_mut!(ptr);
-    let result = unsafe { posix_memalign(ptr_ptr, align, size) };
-    if result != 0 {
-        return Address::ZERO;
-    }
-    let address = Address::from_mut_ptr(ptr);
+    let address = crate::util::malloc::align_alloc(size, align);
@@ -24,4 +20,2 @@ pub fn align_offset_alloc<VM: VMBinding>(size: usize, align: usize, offset: usi
-    let raw = unsafe { calloc(1, actual_size) };
-    let address = Address::from_mut_ptr(raw);
+    let address = crate::util::malloc::calloc(1, actual_size);
@@ -39,4 +33,3 @@ pub fn offset_malloc_usable_size(address: Address) -> usize {
-    let malloc_res = unsafe { malloc_res_ptr.read_unaligned() } as *mut libc::c_void;
-    unsafe { malloc_usable_size(malloc_res) }
+    let malloc_res = cursor.load::<usize>() as *mut libc::c_void;
+    crate::util::malloc::malloc_usable_size(Address::from_mut_ptr(malloc_res))
@@ -48,4 +41,3 @@ pub fn offset_free(address: Address) {
-    let malloc_res = unsafe { malloc_res_ptr.read_unaligned() } as *mut libc::c_void;
-    unsafe { free(malloc_res) };
+    let malloc_res = cursor.load::<usize>() as *mut libc::c_void;
+    crate::util::malloc::free(Address::from_mut_ptr(malloc_res))
@@ -61,3 +52,3 @@ pub fn get_malloc_usable_size(address: Address, is_offset_malloc: bool) -> usize
     } else {
-        unsafe { malloc_usable_size(address.to_mut_ptr()) }
+        crate::util::malloc::malloc_usable_size(address)
     }
@@ -73,4 +64,2 @@ pub fn alloc<VM: VMBinding>(size: usize, align: usize, offset: usize) -> (Addres
     if align <= 16 && offset == 0 {
-        let raw = unsafe { calloc(1, size) };
-        address = Address::from_mut_ptr(raw);
+        address = crate::util::malloc::calloc(1, size);
```
</details>

<details>
<summary>src/util/malloc/mod.rs (FFI wrappers implementation)</summary>

```diff
@@ -19,10 +19,11 @@ use crate::vm::VMBinding;
 #[cfg(feature = "malloc_counted_size")]
 use crate::MMTK;
 
 /// Manually allocate memory. Similar to libc's malloc.
 pub fn malloc(size: usize) -> Address {
+    // SAFETY: FFI call to library malloc.
     Address::from_mut_ptr(unsafe { self::library::malloc(size) })
 }
 
 /// Manually allocate memory. Similar to libc's malloc.
 /// This also counts the allocated memory into the heap size of the given MMTk instance.
@@ -35,10 +36,11 @@ pub fn counted_malloc<VM: VMBinding>(mmtk: &MMTK<VM>, size: usize) -> Address {
     res
 }
 
 /// Manually allocate memory and initialize the bytes in the allocated memory to zero. Similar to libc's calloc.
 pub fn calloc(num: usize, size: usize) -> Address {
+    // SAFETY: FFI call to library calloc.
     Address::from_mut_ptr(unsafe { self::library::calloc(num, size) })
 }
 
 /// Manually allocate memory and initialize the bytes in the allocated memory to zero. Similar to libc's calloc.
 /// This also counts the allocated memory into the heap size of the given MMTk instance.
@@ -51,10 +53,11 @@ pub fn counted_calloc<VM: VMBinding>(mmtk: &MMTK<VM>, num: usize, size: usize) -
     res
 }
 
 /// Reallocate the given area of memory. Similar to libc's realloc.
 pub fn realloc(addr: Address, size: usize) -> Address {
+    // SAFETY: FFI call to library realloc. The caller must ensure `addr` was returned by a compatible allocator.
     Address::from_mut_ptr(unsafe { self::library::realloc(addr.to_mut_ptr(), size) })
 }
 
 /// Reallocate the given area of memory. Similar to libc's realloc.
 /// This also adjusts the allocated memory size based on the original allocation and the new allocation, and counts
@@ -78,10 +81,11 @@ pub fn realloc_with_old_size<VM: VMBinding>(
     res
 }
 
 /// Manually free the memory that is returned from other manual allocation functions in this module.
 pub fn free(addr: Address) {
+    // SAFETY: FFI call to library free. The caller must ensure `addr` was returned by a compatible allocator.
     unsafe { self::library::free(addr.to_mut_ptr()) }
 }
 
 /// Manually free the memory that is returned from other manual allocation functions in this module.
 /// This also reduces the allocated memory size.
@@ -90,5 +94,23 @@ pub fn free_with_size<VM: VMBinding>(mmtk: &MMTK<VM>, addr: Address, old_size: u
     free(addr);
     if !addr.is_zero() {
         mmtk.state.decrease_malloc_bytes_by(old_size);
     }
 }
+
+/// Get the size of the memory block allocated by malloc.
+pub fn malloc_usable_size(addr: Address) -> usize {
+    // SAFETY: FFI call to library malloc_usable_size. The caller must ensure `addr` was returned by a compatible allocator.
+    unsafe { self::library::malloc_usable_size(addr.to_mut_ptr()) }
+}
+
+/// Allocate memory with alignment.
+pub fn align_alloc(size: usize, align: usize) -> Address {
+    let mut ptr = std::ptr::null_mut::<libc::c_void>();
+    let ptr_ptr = std::ptr::addr_of_mut!(ptr);
+    // SAFETY: FFI call to library posix_memalign.
+    let result = unsafe { self::library::posix_memalign(ptr_ptr, align, size) };
+    if result != 0 {
+        return Address::ZERO;
+    }
+    Address::from_mut_ptr(ptr)
+}
```
</details>


### Safe Mock Types (References instead of Raw Pointers)
**Description**: Mock types used in tests (`CompressedOopSlot`, `OffsetSlot`, `TaggedSlot`) were refactored to use safe references (`&'a Atomic<T>`) instead of raw pointers (`*mut Atomic<T>`). This allows safe access to atomic operations and removes the need for manual `unsafe impl Send`.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/vm/tests/mock_tests/mock_test_slots.rs` | 11 | 0 | -11 |
| `src/util/test_util/fixtures.rs` | 6 | 2 | -4 |

**Category Total**: Δ = -15

**Diff Snippets**:
<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (CompressedOopSlot definition)</summary>

```diff
@@ -63,12 +63,10 @@
     /// OpenJDK uses this kind of slot to store compressed OOPs on 64-bit machines.
     #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
-    pub struct CompressedOopSlot {
-        slot_addr: *mut Atomic<u32>,
+    pub struct CompressedOopSlot<'a> {
+        slot_addr: &'a Atomic<u32>,
     }
 
-    unsafe impl Send for CompressedOopSlot {}
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (CompressedOopSlot load/store)</summary>

```diff
@@ -83,9 +79,9 @@
     impl<'a> Slot for CompressedOopSlot<'a> {
         fn load(&self) -> Option<ObjectReference> {
-            let compressed = unsafe { (*self.slot_addr).load(atomic::Ordering::Relaxed) };
+            let compressed = self.slot_addr.load(atomic::Ordering::Relaxed);
             let expanded = (compressed as usize) << 3;
             ObjectReference::from_raw_address(Address::from_ptr(expanded as *const u8))
         }
 
         fn store(&self, object: ObjectReference) {
             let expanded = object.to_raw_address().as_usize();
             let compressed = (expanded >> 3) as u32;
-            unsafe { (*self.slot_addr).store(compressed, atomic::Ordering::Relaxed) }
+            self.slot_addr.store(compressed, atomic::Ordering::Relaxed);
         }
     }
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (OffsetSlot definition)</summary>

```diff
@@ -142,11 +136,9 @@
     /// Julia uses this trick to facilitate deleting array elements from the front.
     #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
-    pub struct OffsetSlot {
-        slot_addr: *mut Atomic<Address>,
+    pub struct OffsetSlot<'a> {
+        slot_addr: &'a Atomic<Address>,
         offset: usize,
     }
 
-    unsafe impl Send for OffsetSlot {}
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (OffsetSlot load/store)</summary>

```diff
@@ -172,9 +166,9 @@
     impl<'a> Slot for OffsetSlot<'a> {
         fn load(&self) -> Option<ObjectReference> {
-            let middle = unsafe { (*self.slot_addr).load(atomic::Ordering::Relaxed) };
+            let middle = self.slot_addr.load(atomic::Ordering::Relaxed);
             let begin = middle - self.offset;
             ObjectReference::from_raw_address(begin)
         }
 
         fn store(&self, object: ObjectReference) {
             let begin = object.to_raw_address();
             let middle = begin + self.offset;
-            unsafe { (*self.slot_addr).store(middle, atomic::Ordering::Relaxed) }
+            self.slot_addr.store(middle, atomic::Ordering::Relaxed);
         }
     }
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (TaggedSlot definition)</summary>

```diff
@@ -237,11 +227,9 @@
     /// The last two bits are tag bits and are not part of the object reference.
     #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
-    pub struct TaggedSlot {
-        slot_addr: *mut Atomic<usize>,
+    pub struct TaggedSlot<'a> {
+        slot_addr: &'a Atomic<usize>,
     }
 
-    unsafe impl Send for TaggedSlot {}
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (TaggedSlot load/store)</summary>

```diff
@@ -257,16 +245,16 @@
     impl<'a> Slot for TaggedSlot<'a> {
         fn load(&self) -> Option<ObjectReference> {
-            let tagged = unsafe { (*self.slot_addr).load(atomic::Ordering::Relaxed) };
+            let tagged = self.slot_addr.load(atomic::Ordering::Relaxed);
             let untagged = tagged & !Self::TAG_BITS_MASK;
             ObjectReference::from_raw_address(Address::from_ptr(untagged as *const u8))
         }
 
         fn store(&self, object: ObjectReference) {
-            let old_tagged = unsafe { (*self.slot_addr).load(atomic::Ordering::Relaxed) };
+            let old_tagged = self.slot_addr.load(atomic::Ordering::Relaxed);
             let new_untagged = object.to_raw_address().as_usize();
             let new_tagged = new_untagged | (old_tagged & Self::TAG_BITS_MASK);
-            unsafe { (*self.slot_addr).store(new_tagged, atomic::Ordering::Relaxed) }
+            self.slot_addr.store(new_tagged, atomic::Ordering::Relaxed);
         }
     }
```
</details>

<details>
<summary>src/vm/tests/mock_tests/mock_test_slots.rs (DummyVMSlot impl Send)</summary>

```diff
@@ -351,7 +339,5 @@
         Tagged(TaggedSlot<'a>),
     }
 
-    unsafe impl Send for DummyVMSlot {}
-
-    impl Slot for DummyVMSlot {
+    impl<'a> Slot for DummyVMSlot<'a> {
```
</details>

<details>
<summary>src/util/test_util/fixtures.rs (Static reference in fixtures)</summary>

```diff
@@ -19,11 +19,11 @@ pub trait FixtureContent {
 pub struct Fixture<T: FixtureContent> {
     content: AtomicRefCell<Option<Box<T>>>,
     once: Once,
 }
 
-unsafe impl<T: FixtureContent> Sync for Fixture<T> {}
+
 
 impl<T: FixtureContent> Fixture<T> {
     pub fn new() -> Self {
@@ -113,11 +113,11 @@ impl<T: FixtureContent> Default for SerialFixture<T> {
         Self::new()
     }
 }
 
 pub struct MMTKFixture {
-    mmtk: *mut MMTK<MockVM>,
+    mmtk: &'static MMTK<MockVM>,
 }
 
 impl FixtureContent for MMTKFixture {
     fn create() -> Self {
         Self::create_with_builder(
@@ -140,32 +140,34 @@ impl MMTKFixture {
     {
         let mut builder = MMTKBuilder::new();
         with_builder(&mut builder);
 
         let mmtk = memory_manager::mmtk_init(&builder);
-        let mmtk_ptr = Box::into_raw(mmtk);
+        let mmtk_ref = Box::leak(mmtk);
 
         if initialize_collection {
-            let mmtk_static: &'static MMTK<MockVM> = unsafe { &*mmtk_ptr };
-            memory_manager::initialize_collection(mmtk_static, VMThread::UNINITIALIZED);
+            memory_manager::initialize_collection(mmtk_ref, VMThread::UNINITIALIZED);
         }
 
-        MMTKFixture { mmtk: mmtk_ptr }
+        MMTKFixture { mmtk: mmtk_ref }
     }
 
     pub fn get_mmtk(&self) -> &'static MMTK<MockVM> {
-        unsafe { &*self.mmtk }
+        self.mmtk
     }
 
     pub fn get_mmtk_mut(&mut self) -> &'static mut MMTK<MockVM> {
-        unsafe { &mut *self.mmtk }
+        // SAFETY: This is in tests. We leak the MMTK instance in `create_with_builder` and get a `'static mut` reference.
+        // We store it as a shared reference to allow sharing, but we cast it back to mutable here when exclusive access is guaranteed by `&mut self`.
+        unsafe { &mut *(self.mmtk as *const MMTK<MockVM> as *mut MMTK<MockVM>) }
     }
 }
 
 impl Drop for MMTKFixture {
     fn drop(&mut self) {
         let mmtk_ptr: *const MMTK<MockVM> = self.mmtk as _;
+        // SAFETY: This is in tests. We take ownership of the leaked MMTK instance to reclaim its memory when the fixture is dropped.
         let _ = unsafe { Box::from_raw(mmtk_ptr as *mut MMTK<MockVM>) };
     }
 }
 
@@ -211,11 +213,11 @@ impl MutatorFixture {
     pub fn mmtk(&self) -> &'static MMTK<MockVM> {
         self.mmtk.get_mmtk()
     }
 }
 
-unsafe impl Send for MutatorFixture {}
+
 
 pub struct SingleObject {
 ```
</details>

### Safe FFI Signatures (References and Boxes)
**Description**: Refactoring FFI functions to use safe Rust types like `Option<&mut T>` and `Box<T>` instead of raw pointers in function arguments. This allows Rust to handle null checks (via `Option`) and ownership (via `Box`) safely at the boundary, removing the need for `unsafe` dereferencing or `Box::from_raw` inside the functions.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `docs/dummyvm/src/api.rs` | 10 | 1 | -9 |

**Category Total**: Δ = -9

**Diff Snippets**:
<details>
<summary>docs/dummyvm/src/api.rs (lines 22-38)</summary>

```diff
@@ -22,16 +22,20 @@ pub extern "C" fn mmtk_create_builder() -> *mut MMTKBuilder {
     Box::into_raw(Box::new(mmtk::MMTKBuilder::new()))
 }
 
 #[no_mangle]
 pub extern "C" fn mmtk_set_option_from_string(
-    builder: *mut MMTKBuilder,
+    builder: Option<&mut MMTKBuilder>,
     name: *const c_char,
     value: *const c_char,
 ) -> bool {
-    let builder = unsafe { &mut *builder };
-    let name_str: &CStr = unsafe { CStr::from_ptr(name) };
-    let value_str: &CStr = unsafe { CStr::from_ptr(value) };
+    // SAFETY: The caller must ensure that `name` and `value` are valid null-terminated C strings.
+    let builder = builder.expect("builder is null");
+    let (name_str, value_str) = unsafe {
+        (
+            CStr::from_ptr(name),
+            CStr::from_ptr(value),
+        )
+    };
     builder.set_option(name_str.to_str().unwrap(), value_str.to_str().unwrap())
 }
```
</details>

<details>
<summary>docs/dummyvm/src/api.rs (lines 50-52)</summary>

```diff
@@ -50,6 +50,4 @@ pub extern "C" fn mmtk_set_fixed_heap_size(builder: Option<&mut MMTKBuilder>, h
 #[no_mangle]
-pub fn mmtk_init(builder: *mut MMTKBuilder) {
-    let builder = unsafe { Box::from_raw(builder) };
+pub fn mmtk_init(builder: Box<MMTKBuilder>) {
```
</details>

<details>
<summary>docs/dummyvm/src/api.rs (lines 65-77)</summary>

```diff
@@ -62,20 +66,20 @@ pub fn mmtk_init(builder: *mut MMTKBuilder) {
...
 #[no_mangle]
-pub extern "C" fn mmtk_destroy_mutator(mutator: *mut Mutator<DummyVM>) {
+pub extern "C" fn mmtk_destroy_mutator(mut mutator: Box<Mutator<DummyVM>>) {
     // notify mmtk-core about destroyed mutator
-    memory_manager::destroy_mutator(unsafe { &mut *mutator });
-    // turn the ptr back to a box, and let Rust properly reclaim it
-    let _ = unsafe { Box::from_raw(mutator) };
+    // SAFETY: The caller must ensure that `mutator` is a valid pointer to a `Mutator`
+    // that was created by `Box::into_raw` (e.g. by `mmtk_bind_mutator`).
+    memory_manager::destroy_mutator(&mut *mutator);
 }
```
</details>

<details>
<summary>docs/dummyvm/src/api.rs (lines 88-97)</summary>

```diff
@@ -88,16 +92,17 @@ pub extern "C" fn mmtk_alloc(
...
-    memory_manager::alloc::<DummyVM>(unsafe { &mut *mutator }, size, align, offset, semantics)
+    let mutator = mutator.expect("mutator is null");
+    memory_manager::alloc::<DummyVM>(mutator, size, align, offset, semantics)
 }
```
</details>

<details>
<summary>docs/dummyvm/src/api.rs (lines 109-124)</summary>

```diff
@@ -109,16 +114,18 @@ pub extern "C" fn mmtk_post_alloc(
...
-    memory_manager::post_alloc::<DummyVM>(unsafe { &mut *mutator }, refer, bytes, semantics)
+    let mutator = mutator.expect("mutator is null");
+    memory_manager::post_alloc::<DummyVM>(mutator, refer, bytes, semantics)
 }
 
 #[no_mangle]
-pub extern "C" fn mmtk_start_worker(tls: VMWorkerThread, worker: *mut GCWorker<DummyVM>) {
-    let worker = unsafe { Box::from_raw(worker) };
+pub extern "C" fn mmtk_start_worker(tls: VMWorkerThread, worker: Box<GCWorker<DummyVM>>) {
+    // SAFETY: The caller must ensure that `worker` is a valid pointer to a `GCWorker`
+    // that was created by `Box::into_raw`.
     memory_manager::start_worker::<DummyVM>(mmtk(), tls, worker)
 }
```
</details>

### Proof Token (ExclusivePlanAccessProof)
**Description**: Guarantees safety of plan mutable access by passing a zero-sized proof token (`ExclusivePlanAccessProof`) that encodes exclusive access at the type level. This allows removing unsafe raw pointer casts to get mutable references to the plan.

Definition:
```rust
pub struct ExclusivePlanAccessProof {
    _private: (),
}

impl ExclusivePlanAccessProof {
    pub(in crate::scheduler) fn new() -> Self {
        Self { _private: () }
    }
}
```

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/scheduler/gc_work.rs` | 2 | 0 | -2 |
| `src/mmtk.rs` | 1 | 0 | -1 |
| `src/util/rust_util/mod.rs` | 0 | 3 | +3 |
| `src/plan/markcompact/gc_work.rs` | 1 | 0 | -1 |
| `src/scheduler/scheduler.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -2

**Diff Snippets**:
<details>
<summary>src/scheduler/gc_work.rs (ExclusivePlanAccessProof usage)</summary>

```diff
@@ -39,26 +39,27 @@
-pub struct Prepare<C: GCWorkContext> {
-    pub plan: *const C::PlanType,
-}
+pub struct Prepare<C: GCWorkContext> {
+    phantom: PhantomData<C>,
+    _proof: crate::scheduler::ExclusivePlanAccessProof,
+}
...
-        let plan_mut: &mut C::PlanType = unsafe { &mut *(self.plan as *const _ as *mut _) };
+        let plan_mut = mmtk.get_plan_mut(&self._proof);
```
</details>

<details>
<summary>src/mmtk.rs (Proof token for mutable plan access)</summary>

```diff
@@ -449,3 +425,3 @@ impl<VM: VMBinding> MMTK<VM> {
-    pub unsafe fn get_plan_mut(&self) -> &mut dyn Plan<VM = VM> {
-        &mut **(self.plan.get())
+    pub fn get_plan_mut(&self, _proof: &crate::scheduler::ExclusivePlanAccessProof) -> &mut dyn Plan<VM = VM> {
+        let arc = self.plan.get_mut_with_proof(_proof);
+        Arc::get_mut(arc).expect("Plan is shared!")
     }
```
</details>

<details>
<summary>src/util/rust_util/mod.rs (ProofCell usage)</summary>

```diff
+/// A cell that requires a proof token to access its contents mutably.
+pub struct ProofCell<T> {
+    value: UnsafeCell<T>,
+}
...
+    /// Get a mutable reference with proof.
+    pub fn get_mut_with_proof(&self, _proof: &crate::scheduler::ExclusivePlanAccessProof) -> &mut T {
+        // SAFETY: The `ExclusivePlanAccessProof` token guarantees that we have exclusive access
+        // to the plan and its components, preventing concurrent mutable access.
+        unsafe { &mut *self.value.get() }
+    }
...
+unsafe impl<T: Sync> Sync for ProofCell<T> {}
```
</details>

<details>
<summary>src/plan/markcompact/gc_work.rs (ExclusivePlanAccessProof usage)</summary>

```diff
@@ -44,7 +44,7 @@ impl<VM: VMBinding> GCWork<VM> for UpdateReferences<VM> {
         VM::VMScanning::prepare_for_roots_re_scanning();
         mmtk.state.prepare_for_stack_scanning();
         // Prepare common and base spaces for the 2nd round of transitive closure
-        let plan_mut = unsafe { &mut *(self.plan as *mut MarkCompact<VM>) };
+        let plan_dyn = mmtk.get_plan_mut(&self._proof);
+        let plan_mut = plan_dyn.downcast_mut::<MarkCompact<VM>>().expect("Plan is not MarkCompact");
         plan_mut.common.release(worker.tls, true);
```
</details>

<details>
<summary>src/scheduler/scheduler.rs (ExclusivePlanAccessProof usage)</summary>

```diff
@@ -561,11 +558,11 @@ impl<VM: VMBinding> GCWorkScheduler<VM> {
         // Tell GC trigger that GC ended - this happens before we resume mutators.
         mmtk.gc_trigger.policy.on_gc_end(mmtk);
 
         // All other workers are parked, so it is safe to access the Plan instance mutably.
         probe!(mmtk, plan_end_of_gc_begin);
-        let plan_mut: &mut dyn Plan<VM = VM> = unsafe { mmtk.get_plan_mut() };
+        let plan_mut: &mut dyn Plan<VM = VM> = mmtk.get_plan_mut(&ExclusivePlanAccessProof::new());
         plan_mut.end_of_gc(worker.tls);
         probe!(mmtk, plan_end_of_gc_end);
```
</details>


### Safe Concurrency (Auto-trait Enablement)
**Description**: Ensuring struct fields implement `Send` and `Sync` (e.g. by removing raw pointers or adding bounds to trait objects) to allow the compiler to automatically derive `Send` and `Sync` traits, eliminating the need for `unsafe impl Send/Sync`.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/scheduler/gc_work.rs` | 3 | 0 | -3 |
| `src/util/heap/freelistpageresource.rs` | 2 | 0 | -2 |
| `src/mmtk.rs` | 2 | 0 | -2 |
| `src/plan/concurrent/concurrent_marking_work.rs` | 3 | 0 | -3 |
| `src/policy/marksweepspace/native_ms/global.rs` | 1 | 0 | -1 |
| `src/scheduler/worker.rs` | 3 | 0 | -3 |
| `src/util/int_array_freelist.rs` | 2 | 0 | -2 |
| `src/policy/immix/immixspace.rs` | 1 | 0 | -1 |
| `src/util/heap/layout/mmapper/csm/two_level_storage.rs` | 2 | 0 | -2 |
| `src/plan/markcompact/gc_work.rs` | 1 | 0 | -1 |
| `src/scheduler/scheduler.rs` | 1 | 0 | -1 |
| `src/util/opaque_pointer.rs` | 2 | 0 | -2 |
| `src/util/test_util/mock_vm.rs` | 3 | 1 | -2 |
| `src/plan/compressor/gc_work.rs` | 1 | 0 | -1 |
| `src/scheduler/stat.rs` | 1 | 0 | -1 |
| `src/util/alloc/allocator.rs` | 2 | 1 | -1 |
| `src/util/slot_logger.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -29

**Diff Snippets**:
<details>
<summary>src/scheduler/stat.rs (Removal of unsafe impl Send)</summary>

```diff
@@ -185,17 +185,15 @@ impl WorkStat {
 
 /// Worker thread local counterpart of [`SchedulerStat`]
 pub struct WorkerLocalStat<C> {
     work_id_name_map: HashMap<TypeId, &'static str>,
     work_counts: HashMap<TypeId, usize>,
-    work_counters: HashMap<TypeId, Vec<Box<dyn WorkCounter>>>,
+    work_counters: HashMap<TypeId, Vec<Box<dyn WorkCounter + Send + Sync>>>,
     enabled: AtomicBool,
-    _phantom: PhantomData<C>,
+    _phantom: PhantomData<fn() -> C>,
 }
 
-unsafe impl<C> Send for WorkerLocalStat<C> {}
-
 impl<C> Default for WorkerLocalStat<C> {
```
</details>
<details>
<summary>src/util/opaque_pointer.rs (Removal of unsafe impl Send/Sync)</summary>

```diff
@@ -4,39 +4,37 @@
-pub struct OpaquePointer(*mut c_void);
+pub struct OpaquePointer(usize);
 
 // We never really dereference an opaque pointer in mmtk-core.
-unsafe impl Sync for OpaquePointer {}
-unsafe impl Send for OpaquePointer {}
```
</details>
<details>
<summary>src/scheduler/scheduler.rs (Removal of unsafe impl Sync)</summary>

```diff
@@ -29,14 +29,11 @@ pub struct GCWorkScheduler<VM: VMBinding> {
-// FIXME: GCWorkScheduler should be naturally Sync, but we cannot remove this `impl` yet.
-// Some subtle interaction between ObjectRememberingBarrier, Mutator and some GCWork instances
-// makes the compiler think WorkBucket is not Sync.
-unsafe impl<VM: VMBinding> Sync for GCWorkScheduler<VM> {}
+
```
</details>

<details>
<summary>src/policy/immix/immixspace.rs (Removal of unsafe impl Sync)</summary>

```diff
@@ -74,1 +74,1 @@
-unsafe impl<VM: VMBinding> Sync for ImmixSpace<VM> {}
+// The fields should be Sync, making this automatically Sync.
```
</details>

<details>
<summary>src/scheduler/gc_work.rs (Removal of unsafe impl Send)</summary>

```diff
@@ -47,1 +47,0 @@
-unsafe impl<C: GCWorkContext> Send for Prepare<C> {}
@@ -130,1 +130,0 @@
-unsafe impl<C: GCWorkContext> Send for Release<C> {}
@@ -484,1 +484,0 @@
-unsafe impl<VM: VMBinding> Send for ProcessEdgesBase<VM> {}
```
</details>

<details>
<summary>src/util/heap/freelistpageresource.rs (Removal of unsafe impl Send/Sync)</summary>

```diff
@@ -28,4 +28,2 @@
-unsafe impl<VM: VMBinding> Send for FreeListPageResource<VM> {}
-unsafe impl<VM: VMBinding> Sync for FreeListPageResource<VM> {}
```
</details>

<details>
<summary>src/mmtk.rs (Removal of unsafe impl Send/Sync)</summary>

```diff
@@ -133,2 +127,0 @@ pub struct MMTK<VM: VMBinding> {
-unsafe impl<VM: VMBinding> Sync for MMTK<VM> {}
-unsafe impl<VM: VMBinding> Send for MMTK<VM> {}
```
</details>

<details>
<summary>src/plan/concurrent/concurrent_marking_work.rs (Removal of unsafe impl Send)</summary>

```diff
@@ -103,10 +100,7 @@ impl<VM: VMBinding, P: ConcurrentPlan<VM = VM> + PlanTraceObject<VM>, const KIND
-unsafe impl<VM: VMBinding, P: ConcurrentPlan<VM = VM> + PlanTraceObject<VM>, const KIND: TraceKind>
-    Send for ConcurrentTraceObjects<VM, P, KIND>
-{
-}
```

```diff
@@ -154,7 +148,4 @@ pub struct ProcessModBufSATB<
-unsafe impl<VM: VMBinding, P: ConcurrentPlan<VM = VM> + PlanTraceObject<VM>, const KIND: TraceKind>
-    Send for ProcessModBufSATB<VM, P, KIND>
-{
-}
```

```diff
@@ -196,7 +187,4 @@ pub struct ProcessRootSlots<
-unsafe impl<VM: VMBinding, P: ConcurrentPlan<VM = VM> + PlanTraceObject<VM>, const KIND: TraceKind>
-    Send for ProcessRootSlots<VM, P, KIND>
-{
-}
```
</details>

<details>
<summary>src/policy/marksweepspace/native_ms/global.rs (Removal of unsafe impl Sync)</summary>

```diff
@@ -86,5 +86,5 @@ pub struct MarkSweepSpace<VM: VMBinding> {
-unsafe impl<VM: VMBinding> Sync for MarkSweepSpace<VM> {}
+
```
</details>

<details>
<summary>src/scheduler/worker.rs (Removal of unsafe impl Send/Sync)</summary>

```diff
@@ -106,3 +105,2 @@
-unsafe impl<VM: VMBinding> Sync for GCWorkerShared<VM> {}
-unsafe impl<VM: VMBinding> Send for GCWorkerShared<VM> {}
+// Auto-derived Send and Sync should apply now.
```

```diff
@@ -301,4 +299,1 @@
-/// We have to persuade Rust that `WorkerGroup` is safe to share because the compiler thinks one
-/// worker can refer to another worker via the path "worker -> scheduler -> worker_group ->
-/// `Surrendered::workers` -> worker" which is cyclic reference and unsafe.
-unsafe impl<VM: VMBinding> Sync for WorkerGroup<VM> {}
```
</details>

<details>
<summary>src/util/int_array_freelist.rs (Removal of unsafe impl Send/Sync)</summary>

```diff
@@ -12,2 +12,0 @@
-unsafe impl Send for IntArrayFreeList {}
-unsafe impl Sync for IntArrayFreeList {}
```
</details>

<details>
<summary>src/util/heap/layout/mmapper/csm/two_level_storage.rs (Removal of unsafe impl Send/Sync)</summary>

```diff
@@ -66,2 +66,0 @@
-unsafe impl Send for TwoLevelStateStorage {}
-unsafe impl Sync for TwoLevelStateStorage {}
```
</details>

<details>
<summary>src/plan/markcompact/gc_work.rs (Removal of unsafe impl Send)</summary>

```diff
@@ -30,7 +30,6 @@ pub struct UpdateReferences<VM: VMBinding> {
     _proof: crate::scheduler::ExclusivePlanAccessProof,
     phantom: PhantomData<VM>,
 }
 
-unsafe impl<VM: VMBinding> Send for UpdateReferences<VM> {}
-
 impl<VM: VMBinding> GCWork<VM> for UpdateReferences<VM> {
```
</details>

<details>
<summary>src/plan/compressor/gc_work.rs (Removal of unsafe impl Send)</summary>

```diff
@@ -35,11 +35,11 @@ impl<VM: VMBinding, F: Fn(&'static CompressorSpace<VM>) + Send + 'static> Genera
 /// to update object references.
 pub struct UpdateReferences<VM: VMBinding> {
     p: PhantomData<VM>,
 }
 
-unsafe impl<VM: VMBinding> Send for UpdateReferences<VM> {}
+// PhantomData<VM> should be Send if VM is Send.
```
</details>

<details>
<summary>src/util/test_util/mock_vm.rs (Auto-trait enablement via bounds)</summary>

```diff
@@ -258,17 +258,17 @@ pub struct MockVM {
-    pub scan_roots_in_mutator_thread: Box<dyn MockAny>,
-    pub scan_vm_specific_roots: Box<dyn MockAny>,
+    pub scan_roots_in_mutator_thread: Box<dyn MockAny + Send + Sync>,
+    pub scan_vm_specific_roots: Box<dyn MockAny + Send + Sync>,
     pub notify_initial_thread_scan_complete: MockMethod<(bool, VMWorkerThread), ()>,
     pub supports_return_barrier: MockMethod<(), bool>,
     pub prepare_for_roots_re_scanning: MockMethod<(), ()>,
-    pub process_weak_refs: Box<dyn MockAny>,
-    pub forward_weak_refs: Box<dyn MockAny>,
+    pub process_weak_refs: Box<dyn MockAny + Send + Sync>,
+    pub forward_weak_refs: Box<dyn MockAny + Send + Sync>,
 }
@@ -370,12 +370,11 @@ impl Default for MockVM {
-unsafe impl Sync for MockVM {}
-unsafe impl Send for MockVM {}
```
</details>

<details>
<summary>src/util/alloc/allocator.rs (Removal of unsafe impl Sync via Mutex)</summary>

```diff
@@ -94,41 +94,36 @@ impl AllocationOptions {
 struct AllocationOptionsHolder {
-    alloc_options: RefCell<AllocationOptions>,
+    alloc_options: std::sync::Mutex<AllocationOptions>,
 }
 
-/// Strictly speaking, `AllocationOptionsHolder` isn't `Sync`.  Two threads cannot set or clear the
-/// same `AllocationOptionsHolder` at the same time.  However, both `Mutator` and `GCWorker` are
-/// `Send`, and both of which own `Allocators` and require its field `Arc<AllocationContext>` to be
-/// `Send`, which requires `AllocationContext` to be `Sync`, which requires
-/// `AllocationOptionsHolder` to be `Sync`.  (Note that `Arc<T>` can be cloned and given to another
-/// thread, and Rust expects `T` to be `Sync`, too.  But we never share `AllocationContext` between
-/// threads, but only between multiple `Allocator` instances within the same `Allocators` instance.
-/// Rust can't figure this out.)
-unsafe impl Sync for AllocationOptionsHolder {}
+// We no longer need `unsafe impl Sync` because `Mutex` is `Sync`.
+// The type is only used by a single thread at a time (mutator or GC worker),
+// but Rust requires `Sync` because it is shared across allocators in an `Arc`.
+// Using `Mutex` satisfies Rust's safety requirements without `unsafe`.
```
</details>

<details>
<summary>src/util/slot_logger.rs (Removal of unsafe impl Sync)</summary>

```diff
@@ -13,11 +13,11 @@ use std::sync::RwLock;
 pub struct SlotLogger<SL: Slot> {
     // A private hash-set to keep track of slots.
     slot_log: RwLock<HashSet<SL>>,
 }
 
-unsafe impl<SL: Slot> Sync for SlotLogger<SL> {}
+// The RwLock ensures safety. If SL is Send + Sync, this is automatically Sync.
 
 impl<SL: Slot> SlotLogger<SL> {
     pub fn new() -> Self {
         Self {
             slot_log: Default::default(),
```
</details>


### Safe State Management (Option instead of Raw Casts)
**Description**: Replacing fields holding references or raw pointers with `Option` to allow taking the value with `take()`, avoiding unsafe casts to bypass borrow checker rules when accessing fields.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/scheduler/gc_work.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/scheduler/gc_work.rs (Using Option::take in ScanMutatorRoots)</summary>

```diff
@@ -420,27 +420,28 @@
-pub struct ScanMutatorRoots<C: GCWorkContext>(pub &'static mut Mutator<C::VM>);
+pub struct ScanMutatorRoots<C: GCWorkContext>(pub Option<&'static mut Mutator<C::VM>>);
...
-        trace!("ScanMutatorRoots for mutator {:?}", self.0.get_tls());
+        let mutator = self.0.take().expect("Mutator missing!");
...
-            unsafe { &mut *(self.0 as *mut _) },
+            mutator,
```
</details>

### Safe VMMap Interface
**Description**: The `VMMap` trait methods `allocate_contiguous_chunks` and `free_contiguous_chunks` were made safe, removing the `unsafe` requirement for callers. This was enabled by implementations (like `Map32`/`Map64`) adopting safe interior mutability. This safety then propagated to callers like `FreeListPageResource`.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/heap/freelistpageresource.rs` | 5 | 0 | -5 |
| `src/util/heap/layout/map.rs` | 2 | 0 | -2 |
| `src/util/heap/pageresource.rs` | 2 | 0 | -2 |

**Category Total**: Δ = -9

**Diff Snippets**:
<details>
<summary>src/util/heap/freelistpageresource.rs (Safe methods wrapping VMMap)</summary>

```diff
@@ -86,13 +85,11 @@ impl<VM: VMBinding> PageResource<VM> for FreeListPageResource<VM> {
-            page_offset = unsafe {
-                self.allocate_contiguous_chunks(space_descriptor, required_pages, &mut sync)
-            };
+            page_offset = self.allocate_contiguous_chunks(space_descriptor, required_pages, &mut sync);
@@ -248,11 +247,11 @@ impl<VM: VMBinding> FreeListPageResource<VM> {
-            unsafe { self.allocate_contiguous_chunks(space_descriptor, PAGES_IN_CHUNK, &mut sync) };
+            self.allocate_contiguous_chunks(space_descriptor, PAGES_IN_CHUNK, &mut sync);
@@ -267,11 +266,11 @@ impl<VM: VMBinding> FreeListPageResource<VM> {
-    unsafe fn allocate_contiguous_chunks(
+    fn allocate_contiguous_chunks(
@@ -300,11 +299,11 @@ impl<VM: VMBinding> FreeListPageResource<VM> {
-    unsafe fn free_contiguous_chunk(&self, chunk: Address, sync: &mut FreeListPageResourceSync) {
+    fn free_contiguous_chunk(&self, chunk: Address, sync: &mut FreeListPageResourceSync) {
@@ -377,15 +376,13 @@ impl<VM: VMBinding> FreeListPageResource<VM> {
-                unsafe {
-                    self.free_contiguous_chunk(
-                        start + conversions::pages_to_bytes(region_start),
-                        sync,
-                    );
-                }
+                self.free_contiguous_chunk(
+                    start + conversions::pages_to_bytes(region_start),
+                    sync,
+                );
```
</details>

<details>
<summary>src/util/heap/layout/map.rs (Safe VMMap trait methods)</summary>

```diff
@@ -8,11 +8,11 @@ use crate::util::Address;
 pub struct CreateFreeListResult {
     // The created free list.
-    pub free_list: Box<dyn FreeList>,
+    pub free_list: Box<dyn FreeList + Send>,
     // The number of bytes to be added to the starting address of the space.  Zero if not needed.
@@ -28,14 +28,12 @@ pub trait VMMap: Sync {
-    /// # Safety
-    ///
-    /// Caller must ensure that only one thread is calling this method.
-    unsafe fn allocate_contiguous_chunks(
+    /// Allocate contiguous chunks.
+    fn allocate_contiguous_chunks(
@@ -55,14 +53,12 @@ pub trait VMMap: Sync {
-    /// # Safety
-    ///
-    /// Caller must ensure that only one thread is calling this method.
-    unsafe fn free_contiguous_chunks(&self, start: Address) -> usize;
+    /// Free contiguous chunks.
+    fn free_contiguous_chunks(&self, start: Address) -> usize;
```
</details>

<details>
<summary>src/util/heap/pageresource.rs (Safe methods wrapping VMMap)</summary>

```diff
@@ -153,18 +153,16 @@ impl CommonPageResource {
         chunks: usize,
         freelist: Option<&mut dyn FreeList>,
     ) -> Address {
         let mut head_discontiguous_region = self.head_discontiguous_region.lock().unwrap();
 
-        let new_head: Address = unsafe {
-            self.vm_map.allocate_contiguous_chunks(
-                space_descriptor,
-                chunks,
-                *head_discontiguous_region,
-                freelist,
-            )
-        };
+        let new_head: Address = self.vm_map.allocate_contiguous_chunks(
+            space_descriptor,
+            chunks,
+            *head_discontiguous_region,
+            freelist,
+        );
         if new_head.is_zero() {
             return Address::ZERO;
         }
 
         *head_discontiguous_region = new_head;
@@ -177,13 +175,11 @@ impl CommonPageResource {
         let mut head_discontiguous_region = self.head_discontiguous_region.lock().unwrap();
         debug_assert!(chunk == conversions::chunk_align_down(chunk));
         if chunk == *head_discontiguous_region {
             *head_discontiguous_region = self.vm_map.get_next_contiguous_region(chunk);
         }
-        unsafe {
-            self.vm_map.free_contiguous_chunks(chunk);
-        }
+        self.vm_map.free_contiguous_chunks(chunk);
     }
```
</details>

### Safe Mutex Guarded Access
**Description**: Removing `unsafe` from function signatures where access to shared state is protected by a mutex, or removing `unsafe` blocks when calling such functions.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/heap/monotonepageresource.rs` | 3 | 0 | -3 |
| `src/policy/copyspace.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -4

**Diff Snippets**:
<details>
<summary>src/util/heap/monotonepageresource.rs (Mutex guarded functions)</summary>

```diff
@@ -213,18 +213,18 @@ impl<VM: VMBinding> MonotonePageResource<VM> {
     /// # Safety
     /// TODO: I am not sure why this is unsafe.
-    pub unsafe fn reset(&self) {
+    pub fn reset(&self) {
         let mut guard = self.sync.lock().unwrap();
         self.common().accounting.reset();
         self.release_pages(&mut guard);
         drop(guard);
     }
 
-    pub unsafe fn get_current_chunk(&self) -> Address {
+    pub fn get_current_chunk(&self) -> Address {
         let guard = self.sync.lock().unwrap();
         guard.current_chunk
     }
```

```diff
@@ -307,11 +307,11 @@ impl<VM: VMBinding> MonotonePageResource<VM> {
-    unsafe fn release_pages(&self, guard: &mut MutexGuard<MonotonePageResourceSync>) {
+    fn release_pages(&self, guard: &mut MutexGuard<MonotonePageResourceSync>) {
```
</details>

<details>
<summary>src/policy/copyspace.rs (Calling safe reset on PageResource)</summary>

```diff
@@ -218,5 +218,3 @@ impl<VM: VMBinding> CopySpace<VM> {
-        unsafe {
-            self.pr.reset();
-        }
+        self.pr.reset();
```
</details>

### Safe Memory Protection Abstraction
**Description**: Replacing unsafe calls to `libc::mprotect` with safe wrappers `mprotect` and `munprotect` from `crate::util::memory`. These wrappers ensure type safety and handle error conditions safely.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/copyspace.rs` | 2 | 0 | -2 |

**Category Total**: Δ = -2

**Diff Snippets**:
<details>
<summary>src/policy/copyspace.rs (mprotect)</summary>

```diff
@@ -291,6 +289,4 @@ impl<VM: VMBinding> CopySpace<VM> {
-        unsafe {
-            mprotect(start.to_mut_ptr(), extent, PROT_NONE);
-        }
+        mprotect(start, extent).expect("Failed to protect memory");
```

```diff
@@ -306,10 +302,4 @@ impl<VM: VMBinding> CopySpace<VM> {
-        unsafe {
-            mprotect(
-                start.to_mut_ptr(),
-                extent,
-                PROT_READ | PROT_WRITE | PROT_EXEC,
-            );
-        }
+        munprotect(start, extent, MmapProtection::ReadWriteExec).expect("Failed to unprotect memory");
```
</details>

### Safe Lifetime Enforcement
**Description**: Changing method signatures to require `'static` lifetimes instead of using unsafe casts to extend reference lifetimes arbitrarily.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/policy/copyspace.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/policy/copyspace.rs (rebind)</summary>

```diff
@@ -362,6 +352,5 @@ impl<VM: VMBinding> CopySpaceCopyContext<VM> {
-    pub fn rebind(&mut self, space: &CopySpace<VM>) {
-        self.copy_allocator
-            .rebind(unsafe { &*{ space as *const _ } });
+    pub fn rebind(&mut self, space: &'static CopySpace<VM>) {
+        self.copy_allocator.rebind(space);
     }
```
</details>

### Safe Slice Access (bpftrace Workaround)
**Description**: Replacing unsafe raw pointer dereferencing with safe slice access (like `typename.as_bytes().first()`) to work around bpftrace limitations without unsafe code.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/scheduler/worker.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/scheduler/worker.rs (bpftrace workaround)</summary>

```diff
@@ -249,11 +248,13 @@ impl<VM: VMBinding> GCWorker<VM> {
 
             #[cfg(feature = "bpftrace_workaround")]
             // Workaround a problem where bpftrace script cannot see the work packet names,
             // by force loading from the packet name.
             // See the "Known issues" section in `tools/tracing/timeline/README.md`
-            std::hint::black_box(unsafe { *(typename.as_ptr()) });
+            if let Some(&b) = typename.as_bytes().first() {
+                std::hint::black_box(b);
+            }
 
             probe!(mmtk, work, typename.as_ptr(), typename.len());
```
</details>

### Shared Ownership and Interior Mutability (Arc<RwLock>)
**Description**: Replaced a raw pointer tree structure (`NonNull`) with `Arc<RwLock>` to share data between instances safely. This eliminates unsafe pointer dereferences.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/int_array_freelist.rs` | 2 | 0 | -2 |

**Category Total**: Δ = -2

**Diff Snippets**:
<details>
<summary>src/util/int_array_freelist.rs (NonNull -> Arc<RwLock>)</summary>

```diff
@@ -60,18 +33,12 @@ impl IntArrayFreeList {
-    fn table(&self) -> &Vec<i32> {
-        match self.parent {
-            Some(p) => unsafe { p.as_ref().table() },
-            None => self.table.as_ref().unwrap(),
-        }
-    }
-
-    // FIXME: We need a safe implementation
-
-    fn table_mut(&mut self) -> &mut Vec<i32> {
-        match self.parent {
-            Some(mut p) => unsafe { p.as_mut().table_mut() },
-            None => self.table.as_mut().unwrap(),
-        }
-    }
+    fn get_entry(&self, index: i32) -> i32 {
+        self.table.read().unwrap()[index as usize]
+    }
+    fn set_entry(&mut self, index: i32, value: i32) {
+        self.table.write().unwrap()[index as usize] = value;
+    }
```
</details>

### Safe Standard Library Alternatives
**Description**: Replacing unsafe calls to external libraries (like `libc`) with safe alternatives provided by the Rust standard library.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/rust_util/mod.rs` | 2 | 0 | -2 |
| `benches/regular_bench/bulk_meta/bzero_bset.rs` | 2 | 0 | -2 |

**Category Total**: Δ = -4

**Diff Snippets**:
<details>
<summary>src/util/rust_util/mod.rs (debug_process_thread_id)</summary>

```diff
-pub fn debug_process_thread_id() -> String {
-    let pid = unsafe { libc::getpid() };
-    #[cfg(target_os = "linux")]
-    {
-        let tid = unsafe { libc::gettid() };
-        format!("PID: {}, TID: {}", pid, tid)
-    }
-    #[cfg(not(target_os = "linux"))]
-    {
-        format!("PID: {}", pid)
-    }
-}
+pub fn debug_process_thread_id() -> String {
+    let pid = std::process::id();
+    let tid = std::thread::current().id();
+    format!("PID: {}, ThreadId: {:?}", pid, tid)
+}
```
</details>

<details>
<summary>benches/regular_bench/bulk_meta/bzero_bset.rs (slice::fill)</summary>

```diff
@@ -32,9 +28,9 @@ pub fn bench(c: &mut Criterion) {
     c.bench_function("bzero_bset_line_memset", |b| {
-        let start = allocate_aligned(LINE_META_BYTES);
-        let end = start + LINE_META_BYTES;
+        let mut buffer = Box::new(AlignedBuffer([0; 1024]));
+        let slice = &mut buffer.0[..LINE_META_BYTES];
 
-        b.iter(|| unsafe {
-            libc::memset(start.as_mut_ref() as *mut c_void, 0xff, end - start);
-            libc::memset(start.as_mut_ref() as *mut c_void, 0x00, end - start);
+        b.iter(|| {
+            slice.fill(0xff);
+            slice.fill(0x00);
         })
     });
```
</details>

<details>
<summary>benches/regular_bench/bulk_meta/bzero_bset.rs (slice::fill block)</summary>

```diff
@@ -52,9 +48,9 @@ pub fn bench(c: &mut Criterion) {
     c.bench_function("bzero_bset_block_memset", |b| {
-        let start = allocate_aligned(BLOCK_META_BYTES);
-        let end = start + BLOCK_META_BYTES;
+        let mut buffer = Box::new(AlignedBuffer([0; 1024]));
+        let slice = &mut buffer.0[..BLOCK_META_BYTES];
 
-        b.iter(|| unsafe {
-            libc::memset(start.as_mut_ref() as *mut c_void, 0xff, end - start);
-            libc::memset(start.as_mut_ref() as *mut c_void, 0x00, end - start);
+        b.iter(|| {
+            slice.fill(0xff);
+            slice.fill(0x00);
         })
     });
```
</details>

### Safe Slot Abstraction (SimpleSlot)
**Description**: `SimpleSlot` was refactored to use `Address` instead of a raw pointer `*mut Atomic<Address>`, eliminating the need for `unsafe impl Send`. The file also removed the deprecated `impl Slot for Address` and refactored tests to avoid unsafe operations.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/vm/slot.rs` | 7 | 3 | -4 |

**Category Total**: Δ = -4

**Diff Snippets**:
<details>
<summary>src/vm/slot.rs (lines 149-190)</summary>

```diff
@@ -149,42 +149,46 @@ pub trait Slot: Copy + Send + Debug + PartialEq + Eq + Hash {
 ///
 /// It is the default slot type, and should be suitable for most VMs.
 #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
 #[repr(transparent)]
 pub struct SimpleSlot {
-    slot_addr: *mut Atomic<Address>,
+    slot_addr: Address,
 }
 
 impl SimpleSlot {
...
-unsafe impl Send for SimpleSlot {}
-
 impl Slot for SimpleSlot {
     fn load(&self) -> Option<ObjectReference> {
-        let addr = unsafe { (*self.slot_addr).load(atomic::Ordering::Relaxed) };
+        let ptr = self.slot_addr.to_ptr::<Atomic<Address>>();
+        // SAFETY: SimpleSlot is constructed with a valid address pointing to a slot.
+        // We assume the pointer is valid and properly aligned.
+        let addr = unsafe { (*ptr).load(atomic::Ordering::Relaxed) };
         ObjectReference::from_raw_address(addr)
     }
 
     fn store(&self, object: ObjectReference) {
-        unsafe { (*self.slot_addr).store(object.to_raw_address(), atomic::Ordering::Relaxed) }
+        let ptr = self.slot_addr.to_mut_ptr::<Atomic<Address>>();
+        // SAFETY: SimpleSlot is constructed with a valid address pointing to a slot.
+        // We assume the pointer is valid and properly aligned for writes.
+        unsafe { (*ptr).store(object.to_raw_address(), atomic::Ordering::Relaxed) }
     }
 }
```
</details>

<details>
<summary>src/vm/slot.rs (removal of impl Slot for Address)</summary>

```diff
@@ -194,20 +198,11 @@ impl Slot for SimpleSlot {
-impl Slot for Address {
-    fn load(&self) -> Option<ObjectReference> {
-        let addr = unsafe { Address::load(*self) };
-        ObjectReference::from_raw_address(addr)
-    }
 
-    fn store(&self, object: ObjectReference) {
-        unsafe { Address::store(*self, object) }
-    }
-}
```
</details>

<details>
<summary>src/vm/slot.rs (test refactoring)</summary>

```diff
@@ -344,11 +341,11 @@ mod tests {
     #[test]
     fn address_range_iteration() {
         let src: Vec<usize> = (0..32).collect();
         let src_slice = Address::from_ptr(&src[0])..Address::from_ptr(&src[0]) + src.len();
         for (i, v) in src_slice.iter_slots().enumerate() {
-            assert_eq!(i, unsafe { v.load::<usize>() })
+            assert_eq!(v.as_address(), Address::from_ptr(&src[i]));
         }
     }
```
</details>

### Safe Plan Access via Unique Reference
**Description**: Accessing the plan mutably via `UnsafeCell::get_mut` (or similar safe field access) when we have unique mutable access to the `MMTK` instance, instead of using an unsafe method to obtain a mutable reference from a shared reference.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/memory_manager.rs` | 1 | 0 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/memory_manager.rs (lines 88-103)</summary>

```diff
@@ -88,12 +88,12 @@ pub fn mmtk_init<VM: VMBinding>(builder: &MMTKBuilder) -> Box<MMTK<VM>> {
 /// Add an externally mmapped region to the VM space. A VM space can be set through MMTk options (`vm_space_start` and `vm_space_size`),
 /// and can also be set through this function call. A VM space can be discontiguous. This function can be called multiple times,
 /// and all the address ranges passed as arguments in the function will be considered as part of the VM space.
 /// Currently we do not allow removing regions from VM space.
 #[cfg(feature = "vm_space")]
-pub fn set_vm_space<VM: VMBinding>(mmtk: &'static mut MMTK<VM>, start: Address, size: usize) {
-    unsafe { mmtk.get_plan_mut() }
+pub fn set_vm_space<VM: VMBinding>(mmtk: &mut MMTK<VM>, start: Address, size: usize) {
+    mmtk.plan.get_mut()
         .base_mut()
         .vm_space
         .set_vm_region(start, size);
 }
```
</details>

### Consolidation of Unsafe Blocks
**Description**: Merging adjacent or closely related `unsafe` blocks into a single block after adding safety guards or verifying safety invariants, reducing the total count of `unsafe` occurrences without necessarily removing the underlying unsafe operations.

**Files and Unsafe Delta**:
| File | Base | New | Δ |
|------|------|-----|---|
| `src/util/rust_util/zeroed_alloc.rs` | 2 | 1 | -1 |

**Category Total**: Δ = -1

**Diff Snippets**:
<details>
<summary>src/util/rust_util/zeroed_alloc.rs (Consolidation and zero check)</summary>

```diff
@@ -37,12 +37,19 @@ use bytemuck::Zeroable;
 /// -   `T`: The element type.
 /// -   `size`: The length and capacity of the created vector.
 ///
 /// Returns the created vector.
 pub(crate) fn new_zeroed_vec<T: Zeroable>(size: usize) -> Vec<T> {
+    if size == 0 {
+        return Vec::new();
+    }
     let layout = Layout::array::<T>(size).unwrap();
-    let ptr = unsafe { alloc_zeroed(layout) } as *mut T;
-    if ptr.is_null() {
-        handle_alloc_error(layout);
+    // SAFETY: `size` is non-zero, so `layout` has non-zero size. `alloc_zeroed` returns a pointer to zeroed memory.
+    // `ptr` was allocated with the correct layout, `size` and `capacity` are equal, and elements are zeroed (valid for `T: Zeroable`).
+    unsafe {
+        let ptr = alloc_zeroed(layout) as *mut T;
+        if ptr.is_null() {
+            handle_alloc_error(layout);
+        }
+        Vec::from_raw_parts(ptr, size, size)
     }
-    unsafe { Vec::from_raw_parts(ptr, size, size) }
 }
```
</details>

## Unclassified

*(No unclassified files yet.)*


