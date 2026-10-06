
# Database idea
I figure this would be what comes of all the work on doa.
Id want it to be columnar, and schemaless (no dedicated schema file or language). 

A 'table' is a folder, columns are ordered sets of values.
Each row in the ordered set is joined to a 'row number' primary index by a reverse index. 
Each column and index gets its own file. 

A column file may look like 
'a', 'e', 'g'

Its reverse index for 8 rows may be 
`[7,2,3],[5,4],[1,8,6]`

An unindexed column could just be the values, in row order.

of course, using plain sorted arrays for indexing makes problems, instead a tree would let us skip the scan over the reverse index and go straight to the rows that point to it. 

at which point, why not make the index the value storage? 
A forward index can exist as well , mapping '5' to 'e' , which could be followed by a reverse index lookup to find other rows with the value e. 

If we're doing things this way, a random primary key might be better than sequential. As long as we know it wont collide, itd avoid us having to store a 'max key' so that a newly inserted row gets pk 'max key'.

in the case where pk's are sequential, the minimum storage space necessary is just [g,a,a,e,e,g,a,g] so 8 values. 
if theyre not its twice that. 
if we use the tree it could be [a:[7,2,3] ... ] etc. which (1 value for len) is 14 values, assuming a flat map, itd grow as the number of keys grew too. 
at least thats searchable.

so just values : 8
value index : 14
value + pk index : 22/30

so be it i guess.

## schemaless
a 'table' is fully described by the file structure of its folder.
a single file may exist storing a single integer representing the row counter, another for a file lock.
each column is accompanied by a forward index if its sorted, containing the address of the value in its series (same logic as block addresses + translators? ) 

neat idea for address space acquisition - what if we do the leftmost byte first, then depending on if the next addition is at the min/max addr we set up the next 8 bit of addresses to be sparse or dense, opening up low or high...

Consider a table

Table Items
col price : int
col inventory : int
col name : text

in a simple row based fs

items.table
0: price,inventory, &name 
1: price,inventory, &name
2: price,inventory, &name
...


alternatively
```
  price.col inventory.col  &name
0 .33     6                &name1
1 .50     4                &name2
2 .25     1                &name3
```

If we want it indexed we can ... 

### Count Index
Store values in a col in order, store the 'count' of items < > = != a value, and use RLE
    Could build a tree on top of this that counts subtree total count. 
    If values are unique, it instead just becomes a sorted array of tuples (val,row#)

### Sorted Index
Store a second column with the values sorted, like
``` 
    price.sorted
    (.25,2)
    (.33,0)
    (.5,1)
```
repeated values result in multiple rows per value. 
updates are cheaper than deduplicated version. 

### Deduplicated Sorted Index
``` 
    price.sorted_set   price.sorted_set.heap
    (.25,(&0,1))       2
    (.33,(&1,1))       0
    (.5, (&2,1))       1
```
How non sized types are stored (strings, arrays, etc) still up for debate a bit.
To pay off values must be appear at least (fat ptr size / val size) + 1 on average.  

### Ascending/Descending
Data is rejected if it isnt the new max/min of the col.
Raw .col is implicitly sorted without having to store the row #. 

### Reverse Index vs Column
When is it cheaper to just store the values in the index only vs copied from the source? 
if we do index only we need a reverse index : row # -> value
This can be unordered or ordered as well. 
Really its just (&,len) per row, so like 8-16 bytes.
We dont need the len even if the data is sized, we can just point at a node in the index directly.
So when the value is larger than a pointer, or whenever its unsized, its free / cheaper.
```
price.rev_idx
&price.sorted
&price.sorted
&price.sorted
```

it needs updating though when the value moves in price.sorted. 

### Translator
Theres a problem though - 
say we have 10 sorted items in price.sorted, and its packed.
If we insert a new item at position 0, everything after it moves, and everything pointing to that needs a repoint.
If we use the address/translator stuff from DOA, the repointing is unnecessary.
If we use a double sided array (in ram) , the writing is fast too. 
So index columns need to store translator params at their start.

So rev_idx.get (rownum) -> addr
price.sorted.translate(addr) -> rownum in price.sorted

Ordered Index
Store values in a tree, mapping value->&`[row number]`  , or just row_number if theyre unique

## Avoiding Migrations? 
I think my main issue is the current schema not being in the code.
Id rather have that be the focus and have a folder of 'migration' scripts written in rust that run to get the table to a new state. 
Each loads the table as it is at the time of writing, uses the library to modify it, then confirms that it matches a 'post' schema before committing or reversing the changes. 

So each migration specifies the before and after schema, and performs the update. The current prod code only knows the current state. 
There should be a reversal function as well... or maybe that could be automatic.
The library should i guess just backup before, attempt the migration, run it, load the table with the new schema, and if it works, delete the backups. 
A Cow file system should be able to do that for free if its just adding columns or renaming them or adding logical constraints. 

### Byte By Byte Addressing
Say we have a 4 byte 'address' 
first 255 items use the most significant byte 

if the 256's addition is an append, we want every subsequent addition in the next 65535 to use a higher address.
So we'd have our previous 'byte' become the LSB (least significant byte) of the subsequent 2 byte address space, and the next address is 0b00000001 0b00000000
If the 256th addition is a prepend, we want everything after this to use a lower address. So we make the previous byte the MSB? 
No its still the LSB but we add 255<<8 to every address? 
so its as if the more significant byte was always '0b11111111' and the new one will be '0b11111110' for the next 256 additions. 

in any other case, we want the new values to go in between the ones that came before, so we just move the byte left and use the less significant bits. 

Lets say we have a series of things that store refs to themselves and a character

[ ('a',0), ('b',1), ('c',2), ('d',3)]

we're using 2 bits. We've reached cap so we need to grow, and allocate an additional bit of our address space. 
The simple case is append. 
[prev .. , 4,5,6,7] 
we just keep counting up. 
for prepend we need 
[0,1,2,3, ..prev] 
so prev needs to interpret previous addresses of 0..3 as 4..7 position. equivalent to adding 4 via io. so when we hand out phys 0 we sub 4 at bitwidth 3 so it wraps to 4. 0b000 - 0b100 = 0b011? 
when its not at an edge, we just rotate our addresses 0..3 left by 1 and place them accordingly. 
`[ ('a',0), ('', 4), ('b',1), ('', 5), ('c',2),('', 6), ('d',3),('', 7),]`
with that we've got a rotate left 1 for pos->addr and rotate right 1 for addr->pos

### Item Movement
Since items may shift due to insertion (ordering must be preserved)
we can take a &mut Vec<(old,new)> to write into to avoid a alloc , the consumers able to reuse the vec.

## Heap Storage

Types that cant be stored in a fixed size slot. 
The block that stores them needs to support slicing so it can return &[u8]
##### Unsorted
If its unsorted thats fine, nothing ever shifts, if we need to update something we try in the existing space or put it at the end and repoint.
No need for translator arithmetic even. 
The referrer stores lengths, we just return pointers to the start of a section.
##### Sorted
We need a index over top,  heap storage without a referrer is kinda useless. The storage itself doesnt store the length of data. 
Its unrealistic to store `Option<u8>` for every byte too. We need a bit tree for free slices. 
It also gets a translator, address -> position, but len needs no translation. 
The referrer is needed to do insertion, so it needs to be ordered to know how to move the items to the left and right when necessary, or the heap needs revptrs. Or the referrer can store prev and next ptrs, like a hashmap + linked list. 

So insertion is seek to position, find space before / after (using the referrer) , move things that need moving, insert the new thing, fix all the pointers with the translator.
So find space : 
maybe a range tree instead of a bit tree? 
0..128 for a len 128 means everythings free. we allocate 8 in the middle and it becomes 
0..56, 64..128

growing the thing is tough isnt it? Since we dont have the lengths they have to come from the referrer. as long as its sorted i guess thats not too bad. just put everything at addr.rl(1) ? 

actually we can just use the referrer+translator to find space cant we? we know len + dist to next, so long as theyre not equal we just keep adding. 

## On Blocks

So , there IS more than 1 type of block

SortedBlock
TreeBlock
GraphBlock (its back!)

Items in a sorted block cannot refer to eachother, but they may be referred to by outsiders. They must be Ord. 
TreeBlock is my bane which i've worked on for months now - nodes can refer to eachother so long as they form a tree. The block needs to specify TreeOrdering. 
GraphBlock - items must be Ord, there is a sorted order to them somehow.

All this is made useful by a sparse binary search algorithm - treeblock uses lookup to find specific nodes, the others need to search by some sort of value. 

Can this translator thing be used for hashmaps ? 
Normally they do rehashing when they grow right? 
I guess we could interpose an address between the hash and the value, so that a user could hold pointers into the hashmap across mutations? We just guarantee we never move things. 
Perf would slightly degrade over time though, as the map grows for items that didnt land on a free slot. 

## Usage

how do i want the usage of this to look? 
```rust 
fn main() {
    //macro reads from dir, infers by filenames and extensions
    let schema = load_schema!("db/inventory");
    //manual
    let inventory_schema = SchemaBuilder::new()
        .name('inventory')
        .col('price', Integer)
            .index(Sorted)
        .col('count' , Integer)
        .col('name' , Text)
            .index(Sorted)
    let inventory = Table::load!("db", inventory_schema);
    
    let cursor = inventory.price.find(0);
    let mut sum = 0;
    while let Some(cursor) = cursor.next() {
        let row = cursor.row;
        sum += inventory.count.get(row) * cursor.value;
        if cursor.value > 500 { break } 
    }
    println!("Value of inventory worth at most 5$ : {}", sum);
}

```

```rust 
mod migration {
//defines prior -> Schema, post -> Schema, run(Schema)
fn run(prior : MutTable) {
    let upns = load_string("upns.csv").lines;
    prior.add_col_with_data("UPN", Text, upns);    
}

fn before() {
    return SchemaBuilder::new()
    .name('inventory')
    .col('price', Integer)
        .index(Sorted)
    .col('count' , Integer)
    .col('name' , Text)
        .index(Sorted)
}


fn after() {
    return SchemaBuilder::new()
    .name('inventory')
    .col('price', Integer)
        .index(Sorted)
    .col('count' , Integer)
    .col('name' , Text)
        .index(Sorted)
    .col('UPN' , Chars(16))
        .index(Sorted)
        .unique()
}
```

Then i suppose a cli tool must exist or something purely to run and reverse migrations, and perform the quick backup and reverse if the post doesnt load. 

```rust 
fn main() {
    //macro reads from dir, infers by filenames and extensions
    let schema = load_schema!("db/inventory");
    let inventory = Table::load("db", inventory_schema);
    let row = inventory.insert(( 1.0, 44, 'potato')); //id like named args here instead. 
    inventory.count.update(row, 43);
}
```

### Under the hood? 

We define our data types , Integer, Text, Chars, etc. 
Col< D : DataType > impls the specific indexes that it can use for that data type. 
Data types really just have to be Heap or Stack, depending on whether theyre variable length or not. 
They all have to be Ord
Then we just impl Index appropriately. 

Is table a struct or a trait? 
i want the type info to propagate so we know exactly what data types are in inventory. if its a struct we'd need to import it right? thatd be weird. Maybe itd be a generic thing? 

Its basically
```
pub struct Inventory { 
    rows : u64;
    pub $colname : Col<$d_type, $idx_type, $unique, $ascending>, 
    ...
}

pub struct InventoryRow {
    pub $colname : $d_type,
    ...
}

pub struct InventoryUpdate {
    pub $colname : Option<$d_type>,
    ...
}

impl Table for Inventory {
    update(rownum, row : InventoryUpdate -> Result<(),DBErr>{
        let i = 0;
        let result;
        for $col in $colname* : 
            if row.$colname.is_none () {i+=1; continue;}
            result = col.insert(rownum, tuple.$i);
            if result.is_ok() { i+=1; } else { 
                //remove rownum from previous cols. 
                break;
            }
    }
    insert
    get
}
```

So the 'table' type is created via macros , but the column types are concrete and implemented generically.

### Leafblock
The pattern 
```
HeapBlock { 
    stack : Block
    heap : (translator, Vec<u8>)
}
```

This is necessary for datatypes that are Heap. Stack datatypes dont need the heap. 
Block gives us a way to lookup a value from the stack, which is a address into the heap
Inserting into the heap requires the stack to find space. 
The heap is sorted. 

For a heap block , running out of space on either the stack or the heap is enough to trigger a split.

## Sorted Block
Simpler alternative to TreeBlock - 
effectively a VecDeque< Option< ( key , val ) > > + a translator. 
Adapts based on append/prepend behavior. 
When ascending / descending is specified we can use a vec instead of a vec deque. 
Maybe that results in a 'bias' generic param. 
Insert doesnt need a relative position, we just take the (key,val) and look it up depending on our bias / binary search. 

Val must be Stack and DType, as must key.
If theyre heap types, then we just use addresses for key or val... 
Maybe Address+Heap is deref? That could be a type. 

So, i think block is a trait, and heapblock is a type of block that implements it. 
heapblock.stack itself can have a heap component, if the key and value of heapblock are both heap. 
```
Heapblock<K,A, LEN_T> { //V is implicitly &[u8]. it could be some thing that impls From<&[u8]>
    stack : Block<K, (A,LEN_T)>, A>
    heap : (translator <A>, Vec<u8>>
}
```

Stackblock<K : Stack,V : Stack> {
    stack : Block<K,V,A>,
}

How do i organize the traits for this? 
```
Col<Dtype : Heap> impls SortedIndex {
    fn sorted_index() -> Index<Heapblock>?
}
```
yeah that works i guess, just specific impls for col gated by its params. 

Table needs to pass down the correct params i guess, when it generates the col/index. 
A col needs handles on its files at a minimum. 
Probably a length too. 

Each column then is a collection of 'something' that holds a cache of live blocks in ram, and a fileptr.
We can call that an Arena right? Maybe BlockFile ? 
For a sorted array its pretty simple, our caching is gonna be terrible though. Perhaps we can store the 'ranges' of each block , just the first and last, in ram, quickly do bsearch on that till we land on the right block, then load it and bsearch within it. 
for 1B records at 65k block sizes, we get what , 2^15 entries in our arena cache? Not bad. 

Do we save that somewhere? Ranges perhaps? For each file, building it otherwise? 
So what, 
price.col.cache ? 
price.col.idx.cache ? 

or is this a sorted idx only thing? i think it is. 
if thats the case, its just another file that sorted idx holds in addition to .heap , it gets .cache

Col contains Arenas
Arenas hold a fileptr and a cache of blocks
Col also knows which arenas are indexes, the table values, etc. 

Table contains cols and the row counter.

# Referential integrity? 
ok so if indexes refer to other columns, then col.update cant update that index.
itd need to be table.update.
likewise if tables indexes refer to other tables, we'd need db.update. 

so how do i do this all in a type forward way...

i can see how sql ended up the way it did. 

db.mytable.price.ordered.find('0.1').first().row


### Layers
theres a division though - 
only things that mutate the db need to go through all the layers that maintain logical guarantees etc.
Reading should still be free to use the specific methods. 
Or the db gives up on maintaining any of those guarantees. 


### Query builder? 
or the db creates a facade? 
let builder = db.query_builder() 
cursor = builder.table.col.find(thing) // this is our user interface, looks like how we wrote things before.
cursor.while(|(row,value)| {
    if value > thing2 {
        builder.output.push(&cursor.value);
    }
    else { 
        false
    }
})
let query = builder.build()
let vals = db.commit(query);

seems possible but gross, basically reinventing sql.


### maybe callbacks

so what, if a col is pointed to by another, updating it requires a &mut to the other too? 
maybe this can use some sort of event listener? 
within the database things can listen for events, and the user can dispatch events through the db. 
events get handled one listener at a time and a listener can choose to absorb or return the event to continue its propagation. 

insert,update,delete, those are basically our events. A read listener would be weird. 
or instead of centralizing it, columns can just store their own listeners right? then its just a function call with the event. 
When the db is built it hands out all the listeners to their dispatchers. 

then we dont have to centralize everything , and we can setup hooks and external listeners as a boon. 

That creates a little problem though - 
an update to a column can mutate *anything anywhere* that listens to it. 

we need rw locks. 

also the callback needs to provide a way to get to the *thing* from the db, and take a &db. 

so inventory.prices is a col
inventory.price_idx() (indexes are read only)
prices.update(&db,row,val) {
    self.assertions.all( Update { row,val,current } ) ?
    let writer = self.writer(row);
    let listener_writers = self.listeners.map(|listener| listener.get_writer(row));
    let current = writer.set(row,val);
    let update = Update { row,val,current }
    let result = || {
        listener_writers.all(update)? 
    }();
    if result.is_ok() { return } 
    else if Err(num) = result{ 
        for i in 0..num {
            listeners[num](Rollback(update)) //rollback vs precheck? 
        }
    }
}

What about transactions that modify multiple columns simultaneously? 
update just has to happen at the table level.
col needs a check_assertions() that runs separately from updates flow, so the table
can make sure that everything is copasetic after the applied changes. 
If it isnt then it rolls back? 
in general its gotta be faster to do 1 write than 2, and potentially 0, so precheck seems better than rollback.

so then : 
a table stores listeners 
    cross column indexes 
    arbitrary user callbacks
    delete/update cascaders
column also stores listeners
    its own index doesnt need it right? thats just internal. 
an index gives out listeners to the columns and tables it refers to. 


so then schema setup looks more like

let schema = schemabuilder::new();
schema.add_table( TableBuilder::name('inventory')
    .col('price', Integer)
        .sorted_idx()
    .col('name', Text)
        .hash_idx()
    .sorted_idx(('price','name')))
db.init(schema)

nah...

how about this. Theres a script that sets up the folder structure and defines the db schema to match it.
Then theres a macro used in the user code that generates all the tables and the db type at compile time, then returns the db type to the user.

# listener structure

lets say we have 2 tables

users
    col name text
files
    col uid Integer idx ForeignSortedIndex(users.id)
    col name
    
this is a bit of a stupid scenario where users doesnt support name lookup but the files for a user do

when a name is changed, files needs to know so uname can be updated. 
if name isnt unique tho how does the index even know what ones to update? 
i guess its 

[(users_id,file_id)]

so when a users_id addr changes (which should never happen)

each listener in users.col.listeners gets (&db, Move{ old id, new id })

## Readers and Writer

so col = db.table.reader().colname.reader()
let row = col.find('value').row;
drop col()
return db.table.reader().row(row)


db.table.writer() -> update,delete,insert

tbh i dont like that approach. 
Id rather have locking / unlocking done implicitly and at the block level.

but i do have to stop the user from deadlocking...
if all the interface fns are atomic...
well a cursor is an i borrow over at least one block in the arena. 

an insertion will require a write to all the columns last blocks and some block within each of the indexes.
i guess my main concern is holding a reader then trying to insert at a block that overlaps with it.

the solution here is a WAL.
An insertion or update anywhere in a column can affect an index anywhere, which can mess up concurrent readers and writers. 

a concern is letting writers run while long running readers are also running. 
columns can store a write ahead log such that append is still possible while a read is ongoing. 
subsequent readers check both the column and the WAL. 
once its the writers turn we just dump the wal into the column.

so, when we get a reader we're promised the column/table wont change while we're looking at it.
the reader must know not to look at the wal.
subsequent writers can do their reads, but changes they make go to the wal.
when a writer is done the wal gets committed to the column.

so whatever our reader/writer is, the 'writer' is actually a reader that doesnt actually promote to a 'writer' until we do something with it that requires that permission.

individual blocks also never bother locking/unlocking. 

a 'writer' isnt handled back until the previous writer is done and its WAL is committed, regardless of how many readers sit between them in the queue, it gets to start reading early. 

come to think of it, technically its not that it has to wait after attempting to write for a writer, its that it has to wait after attempting to read after it wrote.
if something else refers to the table being written to via a FK that may need updating
so if a listener requires getting a writer, its a sneaky deadlock waiting to happen if the caller holds another reader/writer to the referring table. 
so what a writer then claims both the table and any pointing tables? 


## Singleton
ALSO i think db should be a singleton bcuz idk how else we're passing it all over the place. 


