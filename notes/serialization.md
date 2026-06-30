Ok starting with the variable length keys , i wanna do this like rykv using relative offsets to point to variable length data. 
Also maybe borrowing from freelists a little bit. 

[INode : discriminant : u8, nkeys : u8, keypos : [u32; N], children : [usize;N+1] , capacity: u32, len : u32] | [capacity bytes DATA[]]
[LNode : discriminant : u8, nkeys : u8, keypos : [u32; N], vallens : [u32;N] (if Val is Not a fixed size), padding : [u8;N] capacity : u32, len : u32][ (Key|Val|pad)[] bytes] 

also maybe i can support non-unique keys by appending the index to the key. so abc1 is distinct from abc2 even though the user only put in abc twice. 

The point is to make a nice database index using this pattern, 