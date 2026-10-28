# Structure
The most recent top level entries are towards the top.

moved verbatim from crates/doa/notes/devlog.md 2026-10-27, block-substrate sections only.

# Review
blocks.rs
why is ptr a u32? if its a relative pointer id expect a new type, but u32 is too small isnt it? 
fixing the block header size to 16 bytes is ... not strictly necessary ? It could be some other size couldnt it? 



# What must be done


## 0. Consider address newtypes
H<P> and S<P> are newtype wrappers over the intra block pointer type P. 
Since P is a byte address it may have to be wider than the A used by the structures that will live in block. 
If we're aiming for 4096 byte blocks, we'll need u16s. 
How these pointers are used is different - mainly, H is relative to the back of the block. 

xxxxxxhi

A H pointer to the last 2 would be (2,2) as (A,A) . Block.cap-H.0..Block.cap-H.0+H.1 is the region pointed to by H. 
Doing that makes sure we dont accidentally pass a stack pointer to the heap and break things.

A stack pointer is just the byte index into the stack. Idk if it should store len or not.

## 1. Create Block
Block<P> as a dual-front allocator over fixed size memory.
Block stores stack_len, heap_len, cap, ptr
cap is SIZE - padding - sizeof<Block<P>> bytes, so the serialized form with Block|padding|Data is exactly SIZE bytes

    block.stack_push(T)->Result<A>
    block.stack(A) -> &T
    block.stack_slice()[A..A] -> &[T]
    block.stack_shrink(Pos) //pops down to Pos, shrinking stack. assume pos..stack_len is already dropped.
    block.alloc::<T>::(len)->Result<H>
    block.heap(H) -> &[u8]
    block.free::<T>::(len)
    block.allocator(&mut self) -> Allocator

To grow a block the consumer would have to create a new one then copy the items over one at a time.
Actually a growable block is possible, but it would be built over Block, maybe BlockVec.  
Allocator is a restricted view of the block that only allows alloc.
it'll be used by archive instead of a &mut Vec<u8>

the block has to mind padding and alignment for all the types it stores, hence the fn level generics. 
it might need more of those actually, i only put them on the write methods.

i refer to block as store in later (previous chronologically) entries.

## 2. Consider Relptr
The connection between Block and its data has to be a relptr for Block to be serialized, since we don't know where itll end up when deserialized. 
Initially though it'll be born on the heap, so the Block struct has to be heap allocated as well and pinned, then the offset stored relative to the block structs current position.

or is it a waste? Can we just update that one pointer when deserializing it from a known padding based on T and size?
Do we need a pointer at allll idk i want opinions. 

## 3. Split Archive , RelOrd
Archive's single concern should be shoving K into a block. 
it doesnt need to require K to be ord, thats a concern of blockset etc.
cmp_key(&block, &handle, &K) -> Ord
have to keep in mind that handle can contain multiple H, for a complex type with multiple heap allocations. 
itd have to implement cmp key by going through each of its fields and cmp_key against K's field of the same name where each is also ICmp
the primitive types we implement it on wont have to do that though

## 4. Archive::Heap const bool
consider the future where we can derive impl archive on a struct where all the fields are also archive. 
The archive fn as it was

```rust 
    ///append k's heap bytes, return its handle. panics past a 4 GiB heap
    fn archive(k: &Self, heap: &mut Vec<u8>) -> Self::Handle;

    //example of a higher order archival - Vec<HeapType> needs to do this
    ///append k's heap bytes, return its handle. panics past a 4 GiB heap
    fn archive(k: &Self, alloc: &mut Allocator<P>) -> Self::Handle<H> {
        let names : Vec<H> = vec![];
        for name in self.names {
            names.push(allocator.alloc(name));
        }
        let nameptr = allocator.alloc(names);
        handle.names = nameptr;
    }
```

the question arises , do we need a separate trait for primitive types and secondary types?
maybe a const bool on T indiciating whether or not it heap allocates? 
maybe alloc_slice should exist to simplify the type stuff, since itll be doing size_of<T>(), a &[u8] is 16 bytes no matter what right? 
so if we do that then we branch on whether or not the type within V heap allocates
if it does we individually archive each one first, get H for each, then alloc for the Vec<H> and store that H in the handle.

## 5. The Archive derive Macro

### 5a Create Stack
Stack is the simplest form a structure utilizing a block can take - we can only push and pop, and cant mutate any elements.
push gives you back ID(BlockID,Address), it archives T and pushes it onto the end. 
If it doesnt have space, it returns an error. 

### 5b serialize stack to a file, then load it
    as said, save a stack to a .block file and load it.
    lets set the block size to 4096 bytes.

### 5c Manually create a few composite types, impl Archive on them
    ```rust 
    struct A {
        a : i32,
        b : f32,
        c : (i32,f32)
    }

    struct B {
        a : Vec<i32>,
        b : String
    }

    struct C {
        s : Vec<String>
        h : i32
    }

    struct D {
        a : A
        c : C,
    }
    ```
### 5d The Derive Macro
    take what the pattern reveals and make a derive impl for archive that works on all of them. 

### 5e. Create Stacks
stacks is a Vec<Option<Stack>>
if the last block doesnt have space it appends a new one.
once an item is in the block its immutable (for now).
stacks manages a view over a file storing an array of Stack. 
Stacks has a budget of blocks its allowed to keep in memory.
It tracks how many readers there are for each Option<Block>.
It doesn't operate in thread - A single thread owns the sync state which acts like a RWLock over all the 
blocks while operating in budget. It recieves requests and responds through a channel. 
The consumer doesnt see the channels, it just uses the stacks struct it was given which has the channels hard wired into it. 
Even if stacks is cloned, the single consumer is the thread owner so mpsc still works out.  
When stacks is created that thread is spawned, it lives until stacks is dropped. 

come up with some fair benchmarks to test its overhead and investigate performance issues and regressions. 

## 6. Port Blockset
this is likely parallelizable with 5, maybe make a worktree though and merge back after.
blockset should be ported to use Block.
Its mask should be a stack allocated buffer on the bottom, preallocated with enough space to mask cap of T. 
When blockset runs out of space, getting an error from grow/push/alloc on block, it should return the seek'ed position that K was going to go at,
and error.
The holder can choose to grow it or make a new one or split it or whatever. 
a constructor should be created that takes a block that already exists, and one that creates the block new. 
The one that builds onto an existing block will be used for skipset. 

Yk what, maybe the block should be created with a runtime max-capacity for block, and it auto grows up to that, growing the block.
So to control whether the block grows or not we just create it with a max capacity of our desired size. and if block is already that size itll never grow. 
of course the set within the block grows up until it hits the heap bottom or fills the block. 





# Summarizing the previous sections' conclusions

Store must exist
Allocator must exist as a bridge between the block and a Archive type's archive fn.
Relptrs must exist for the connection between the block header and the block data,
Consideration must be given to the padding in the blocks layout 
The heap, growing in reverse, requires a ptr type container HeapPtr(ptr,len).
The heap's alloc fn must be generic over some T where T : Sized - its allocating a slice of T so it can compute alignment and padding.
Archive does not have cmp_key, but handles and views are comparable
Archive doesnt take references to vecs, it takes an allocator, which is a mutable view of the blocks heap
the blocks mask must be capable of masking the entire contents of the block if heap.len were 0.


## Intended Operation

BlockSet, BlockMap, BlockBimap are good prototypes, and have a legitimate usecase.
The new structures will not be their consumers, but they will serve as a baseline. 
Lets call them BlockTable and BlockIndex.

```rust
    #[derive(Schema,Archive)]
    #[nonnull]
    pub struct MyTable {
        #[index] //an index file is created containing a map of name->row id
        name : Text,
        country : [char;2],
        age : u32,
        #[index nocopy] //the column is replaced by an address (BlockID, BlockAddr) to a position in the created index.
        children : SortedVec<ID>,
        weight : f32
    }

    fn main() {
        let mut table = MyTable::open("data/MyTable/");
        let mut cursor = table.children_idx().cursor(); //fields labeled index get a struct impl yielding the index
        let mut refs = cursor.range(&[ID(1)], &[ID(2)]); //an iter over everything that's first child is 1 
        let cum_wgt = refs.map(|id| table[id].weight()).sum()
        println!("{}", cum_wgt);
    }

    //alternatively

    #[derive(DBSchema)]
    struct DB {
        mytable : MyTable
    }

    fn main() {
        let db = BlockDB::<DB>::load("db/");
        let mut weight_sum = db.query<f32>(|db| {
            let table = db.mytable().read();
            let mut cursor = table.children_idx().cursor(); //fields labeled index get a struct impl yielding the index
            let mut refs = cursor.range(&[ID(1)], &[ID(2)]); //an iter over everything that's first child is 1 
            let cum_wgt = refs.map(|id| table[id].weight()).sum();
            return cum_wgt;
        });
        println!("{}", weight_sum);
    }

    //that approach sets up a database schema so we have a typesafe way to get tables
    //and lets db run on dedicated threads for caching efficiency, and in a io_uring enabled runtime. 
```

that doesnt really actually pertain to what we're doing right now much, now that ive written it. 
it does make the scale of the macros necessary to do something like this a bit clearer but regardless

Table : IDs are immutable, stable. 
Index : Refs are (BlockID,Address). 
Identifiers dont change - the block an item is in may change though. 

Both table and index will utilize Block
block is a fixed-byte-size datastructure that features a stack : [T] growing upward from 0 and a Heap growing downward from len.
Heap is referenced by HPtr(A,A). A block with size 12 with u8 A and storing a single key "hi" may look like 
[stack_len : 2, heap_len : 2, relptr : 0 | PAD | (2,2) x x x x 'h' 'i']
cap and pad are available as const fns derivable from size and T. 

store thus provides means of growth and access for the stack and heap - 
store.stack().push(T)->Result<A>
store.stack().push_buf<T>(len) -> Result<A,len>
store.stack()[A] -> &T
store.stack()[A..A] -> &[T]
store.heap().alloc::<T>::(len)->Result<HPtr>
store.heap()::<T>[HPtr] -> &[u8]
store.heap().free<T>(len)

it returns an err on alloc or push when it would cause stack_len + heap_len > SIZE

actually i reconsider making stack a Vec<T>, if we're doing the work to make heap work with alignment and different sized types, 
i think stack gets the same treatment.

then blockset can be built over it with minimal additional difficulty.
instead of the struct holding Vecs it holds a RelBox<Store> then ptrs into that store for mask and data. 
the caveat is that unlike alloc, stack.push requires T to be sized. 

im a little unsure on using A for the stack ptrs. mixes up with position. 
what if pos isnt a usize but instead we have Addr(A) and Pos(A) and Pos(A) derefs to usize whereas address A derefs to A. 

anyway just have to build an allocator for a fixed size block that manages padding and sizing and makes sure the stack and heap dont overlap, and does the relative
pointer math for the heap no biggie /s. 

once thats there blockset can be ported to use store.
also the relptr bit - there because the simplest way to do this i think is to have the allocation be SIZE-sizeof(StorageBlock).
Then we pin storage block, and write its relptr as the address given by the system allocator - pos of self. 
When we serialize the Storageblock we update the relptr to data to be 0, indicating its immediately after StorageBlock.

One concern i have is with stack realloc for resizing the allocation for a pointer. 
it should know when an item is the last i guess. 
i guess i just want to prevent confusion where we try to grow the mask that lies under the set.
or anything else growable for that matter. 
either mask and skiplist preallocate their maximum space consumption or i use the heap right? 
both are very precomptable. 
i can even link their sizes to SIZE and T so they double when and if the StorageBlock grows. 
cant do that if theyre on the heap. 
maybe instead of grow, we just make a new bigger block and push all the items into it with their new desired size and order. 

so BlockSet::new(size) {
    block = malloc(size);
    mask = mask::new::<T>(block, len);
}

as well as new_with an existing block for the skiplist consumer 
BlockSet::new_with(block) {
    mask = mask::new::<T>(block, len);
    data = block.push_buf::<MaybeUninit<T>>(2);
    let mut new = Box::new(BlockSet { fixed size stuff, block, mask, data })
    new.block.set_base(&self); //setup relptr
    return Pin::new(new);
}

doesnt it have to be pin? 
I havent messed with that but since blockset.block is a relptr i think blockset does need to be, hence why the box.  

## Structure

table<T>
1 .data file for blocks : [(Block<T>, [u8;SIZE-sizeof(Block<T>])]
we maintain a Vec<Option<Box...>> and set unloaded blocks to none. 
a T's ID is (BlockId,Addr) so items added to the block dont have a strictly +1 relation to the previous. 
a table is guaranteed to be append only so theres no deltas to apply.

index<K>
1 .index file for a blockset<(MinK,BlockID)>
1 .data file for blocks, loaded in ram as : Vec<Option<Box<BlockMultiMap<K,(BlockID,A)>>>> //k isnt unique, multiple rows per K
    if k is unique then instead its just a block map. 

Block {
    stack_len
    heap_len
    cap
    relptr
}

actually is there a point in not inlining block data into the block? The thing relptr points to. 
originally it was so we can alloc that separately , keep box on the stack. 
but doesnt Box(Block,BlockData)> basically just do that?
nah the point is so data can have a variable size without const generics. 

## RelOrd (settled on name)
secondary trait in addition to archive
rel_cmp(&self, &heap, &handle) -> Ordering
the derive macro of archive on complex types does not impl RelOrder, its not necessary for archive, since archive is also for slab-allocator style
tables. Its necessary for our primitive types so we impl it on them manually. 
A composite type could have it impled upon, then we could use a derive macro so long as the members of the composite type are also all RelOrder

# More General? 
maybe the stack vs heap analogy is going to far - really its a means of 2 growable regions coexisting non-overlapping in the same memory space.
the other way i can think of is striding - but that compromises on O(1) access or wasting space.
The heap needs proper allocation though - wait im a fuckin genius
The only things that reference a blocks heap are its own stack
So theoretically we could revive the leafblock pattern from long ago, where the heap was stored in order and sparse, and each stack item was (ptr,len)
so by scanning it we could find nonzero values of (b.ptr - a.ptr) - a.len
would certainly be faster for lookup - however, im tempted to just have O(BLOCK) removal and O(1) heap insert. 

that strategy though - idk how we could be generic over it. 

regarding our block
BLOCK<SIZE> { MASK } | PAD | STACK | HEAP
the entire thing should take SIZE bytes and be SIZE aligned.
size must be a power of 2. 
the size of the mask is precomputable - SIZE/sizeof(T) rounded up to a multiple of 64 then divided by 8, assuming no heap allocations for the 'worst case'.

i think i have to have STACK and HEAP be views.
To the compiler its gotta just be 
BLOCK<SIZE> { MASK[u64; <M/64>] } | [T;N]
where M is N/64 rounded up, N is precomputed.  
ill presume thats possible with macros because i want it to be lol.

limitations - 
- changing T renders the old file unsafe to load, padding bytes may become part of the struct so itll be corrupted.
- any item must not heap allocate more than sizeof T * N-1 bytes - a max size for our variable sized types.  

Store Trait Boundary 
trait Store<T,A> {
    fn stack(&self) -> &[MaybeUninit<T::HANDLE>]
    fn stack_mut(&mut self) -> &mut[MaybeUninit<T::HANDLE>]
    fn extend_stack(&mut self, cap) -> Result<OK, max_possible>; 
    fn heap(&self, ptr, len) -> &[u8]
    fn push_heap(&mut self, &[u8]) -> Result<OK, max_possible>; 
    fn pop_heap(&mut self, len) -> Result<OK, or err if len is too large>
    fn bytes(&self) -> &[u8]
}

issue - archive's handle needs a ptr to construct, we need to know insertion is possible before doing it.
block set cant just assume it can grow.
what if we do the heap push first, then feed into block.insert(handle, available capacity)
oh interesting, so blockset doesnt manage its own capacity. we tell it its capacity. 
or, no, hmm. there has to be feedback. 
i guess we can pass it max capacity? 

normally a vec stores ptr, len, cap
our store will be stacklen, heaplen, cap as A.


## Doublevec reappears?
im realizing this is maybe a specialization of something more generic? 

struct DoubleArray {
    front_len
    back_len
    cap
    ptr
}

impl DoubleArray<A,B> {
    front / front_mut(&mut self) -> FrontMut<'_, A>
    back / back_mut(&mut self) -> BackMut<'_, B>
    grow(self, amount) -> Self
    try_grow(self,amount) -> Result<Self>
}

struct Front { 
    &array // just a view
}

//impl traits for vec-like functionality. 
impl [Index, Slice, Iter] for struct Front {
}

and if manual grow is impled on array then we could use that to build a double vec over it. 
since its not inline though ptr would have to be a relptr?
eh not sure thats useful or necessary.

relptrs are a missing link i think, im just inlining because i dont have them i think.
and that gets me padding...

idk i should just be fine with fixed size blocks, its the only way the file is addressable. 
actually is that true? 

## Large Text?

the index stores (min, addr), since the blocks arent  in order it has to store addr. 
if addr isnt just +1 each time by width of block, but instead blocks...
can they do translator stuff to be growable? 
no screwing with the translator would put space between every block, or modify all addresses. 
itd be easier to just copy paste the block at the end and make it wider...
kinda stupid tho.
itd be better to store large byte strings by chunking them. 
rejected then. 


## Relptrs

how do i allocate it anyway?
how do i communicate that to the type system? 
I have to make the thing then communicate it right?
An i need them anyway for complex structures. 

i guess if its 
struct Relptr<T> {
    offset : usize
}

impl Relptr {
    fn dref(&self, base: usize) {
        *((base + self.offset as *T] ;//T
    }
}

so then for block serialization, deserialization is certainly zero copy but serialization cant be... the first alloc the OS gives is isnt gonna be relative. 
but we can make it relative. 
just have to update the ptr when we save the block. 
which  means when archiving, we manually add padding after the struct header so the items are properly aligned when we load it. 

## Complex Archive Types

handle currently works for simple types - 1 heap allocation per handle. 
but the trait needs a split up. 
either we do individual ptrs for each item in the handle, or the handle points to one continuous allocation and inside we have subslices. 
idk if theres any benefit to putting them all in one then storing sub-pointers. just more arithmetic to screw up.

if we were to order the heap there'd be a point. itd let us move the whole allocation at once. 
but how ... do we handle this. 
we dont need a vec of ptrs... this is very literally rkyv's archive trait.
except we're splitting the archived data into 2 parts - heap and stack. 
so the function that builds the handle needs access to the allocator. 
it needs to 'push' each vlen data onto it and get a pointer back then store that into the handle. 
the pointers in the handle though arent relptrs, theyre BHeap (BlockHeap) pointers. 

I kinda dislike how the archive trait is lifetime blind of the block... idk what to do with that though.
maybe some phantom covariant or something...

maybe instead of taking &[u8] as the arg for heap, we take &BHeap so the offset cant get messed up. 

## Handle
I think the reason i hesitate about handle is because its combining cmp_key in. 
id rather another trait exist , or a set of traits, for wrapping Ord or Add or whatever other functinality that take &other to instead do
(&Ablock, Relptr<A>, &BBlock, Relptr<B>)

archive should be about archiving, a separate family of traits can exist doing std library things in block space. 
i mean really a 'handle' is relative to the block its in, its identified uniquely by an index of the block's stack, which also provides the base ptr. 

perhaps it should be built atop an 'allocator' , the archive fn uses the allocator to request space, gets a ptr for the amount it wants (or err and bail)
then shoves its bytes into there, and if theres multiple fields the function alters the offset manually for each that it stores into handle. 
but THOSE might need padding right?
so i need to distinguish between primitive types (bool, byte, int, str, date) and secondary types - Vec<T>
each should know its size and alignment. 
so archive needs a function to compute the size of the heap allocation needed to factor in the padding between variable length fields. 

for example 
```rust 
#[derive(Archive)]
struct customer {
    name : String,
    cards : Vec<usize>
}
```

cards needs to be 8 byte aligned, if name's len isnt a multiple of 8 , it wont be with a naieve dump of the bytes if they come after name. needs padding.
alternatively, we let the allocator handle padding, and have each alloc<T>(len) handle the padding.  

i think thats kinda nice actually, it makes Vec<Vec<usize>> easier, and makes the derive impl code simpler. 
we just archive the inner vecs first, building an outer vec of BHeap pointers, then alloc space for that. 

## Trait Changes

cmp_key exists as a bridge between the archived type and the native type.
2 archived handles are technically not comparable, maybe their views are but not the handles. 
what if view.cmp(K) exists
then we just bound the View as a GAT. 
Block provides a fn view that takes an index, gets the handle, and uses it to provide the view. 
We dont want to go through the trouble of building a new data type from the handle. 
view is a transformation.
it maps the relative pointer stored in the handle to an absolute pointer, relative to the base of the block its in. 

what if handles are comparable, but we add a 4th meaning. lt, gt, eq, peq for partially equal. 
peq means that the 2 need a deeper comparison.

if that happens we build the view type, and compare K::as_view() against K::view(&Handle, heap : Heap)


## Constraints
So if this is to become a database table or index, then a block type has to exist that drops the bound K::Ord. 
its a bonus for a block that just wants the heap management without bothering to order its contents.
Then additional indexes can live alongside it. 
I was also thinking earlier - sorting is great and all, but only for 1D data. 
next/prev both imply a position on a number line that can advance or retreat. 
a higher order index could just provide 'nearby'. each item stores the closest other items to it. 
you have to store a list of the addresses youve visited though, but that can be O(1) with a hashset. 

also, the 'blocks' portion of the arena, the continuous memory storing an array of fixed size blocks. 
for an index, has to exist an index above that which stores a copy of the minimum element of each block.
thats necessary even if the block is unordered, because not every block stores the same number of items. 
min(k) for a table where the K is the primary key (ie just a row number) is still needed assuming the data type needs any heap storage.
a fast path can exist if it doesnt, that can be const bool on the archive trait impl.

that raises another point tho- in the BlockTable min K isnt correct. we dont care about copying the first item, we need the row number. 
we can potentially have a block table where indexed columns are instead just stored as an address to the index. 

if tables map to folders, we can put a definition.hash file in there so they cant be opened with a wrong schema by accident. 

the confusion between block address and arena address is annoying, lets call them block_id and address respectively. 

## Table Constraint - Need Side Files for Mutable Heap Types

idea first - fields that update frequently should probably be indexed nocopy in the table, otherwise theyll apply heap pressure
actually if we want 'blockid,addr' to be the stable immutable handle by which a table row is identified, we cant allow heap allocated values on the table to change unless theyre stored in a side-file. 
we'd need a different type - IString to allow block local heap allocation. since it would never be able to change. 
mutable heap allocated types require a side file - if the update is larger than the original in an already full block there's nothing we can do. 
lets call that a .heap file - not blocks, just a big slab allocated region. 

regarding allocators, i was thinking a freelist on the heap might be helpful. (next,len) as at the heap bottom , where len is the number of bytes in this free region. if A >= size, no free space, have to lower the heaps bottom. 


# Array Block Convo

not many tokens left today so no implementation, but im considering how the serializable block arena could be layed out.

blockset stores deltas, mask, heap, and data - those are its allocations.

if each block in the arena is a fixed size allocation, mask becomes fixed size.
deltas can be taken out since the arena can own it, and prevent modification wholesale until deltas is used.
heap and data though - i think those need to take on the behavior of an ordinary memory space. heap grows down, stack grows up, when they meet neither can grow.

its gotta be inline though. im thinking we just store data=[u8;SIZE] , then have fns .stack(&self)->&[MaybeUninit<T::Handle>] and .heap(addr,len)->&[u8] with alloc and grow_stack both increasing their
portion by some amount, refusing when they meet or ask beyond the capacity.

we need some custom Store trait that facilitates all this in a way that lets the blocks be generic over it.
a pure memory one could actually just be different vecs,
a mem mappable one could do the usual trick of offsetting heap ptrs from the ptr::MAX() , so store.stack(ptr,len)->data[ptr..ptr+len] , store.heap(ptr,len) =
store.data[store.data.len()-(ptr::MAX()-ptr).. that + len), and data can grow by moving the heap allocated section to the new back.
a fixed size one skips the offset trick and refuses when it runs out of space.

so archive needs to support a bit of functionality - mainly getting the size of the heap allocation before archiving it.

# Array Block
the composition piece for arenaset (blockblock?)
split construction : 
colname.index
colname.data

correspond to 
Arena
Vec<BlockData>

struct ArenaDeltas ; //can express that a key moved across blocks, shared between blocks. 

struct Arena { 
    skip : SkipMap<T,BlockHeader<T>,u32>
    deltas : ArenaDeltas //shared reusable memory for updates between all blocks, blocks get a bool dirty guard instead. 
}

struct BlockHeader<T,BA,SIZE> {
    data_addr : u32,
    len : BA, //num bytes occupied in block
    rev : bool,
    block : Option<Box<BlockData<T,SIZE>> 
}

struct BlockData<T,BA,SIZE> {
    translator : Translator<BA> 
    prev_pos :  BA
    streak :    BA
    avg_pos :   BA
    epoch : u8
    mask : [u64; SIZE/64] // could be made smaller if we know T::HANDLES size, only need as many bits as handles fit in the block . 

    stack_len : BA
    heap_len :  BA
    data : [u8 ; SIZE]
}

we need a vec alternative - a miniaturized program memory space with a fixed size
stack and heap grow until heap_len + stack_len*sizeof(T::Handle) >= SIZE
the heap grows downwards from the top of the memory space, the stack grows up from the bottom

the contents of data is heap and data from blockset both sharing the same memory space. 

i think we need a unifying store trait that supports 
alloc(len) -> &mut [u8]
grow_stack(amt)
free(BA,BA)

actually thats just an allocator, maybe we can get one and plug it into both the heap and stack vecs we already use. 
only slight complication is that the heap needs to grow backwards. 

also block needs to be sensitive to errors from the vec too now, it will refuse once the block is full which isnt at a fixed number of items 
but depends on the size of the heap allocations of those items too. 
so we have to check their size before even trying to insert them. 
