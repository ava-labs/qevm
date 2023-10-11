pragma solidity ^0.8.0;
  
contract HelloWorld {
    uint32 x = 0;
    function hello(uint32 y) public returns(string memory, uint32){
        x += y;
        return ("Hello, World!", x);
    }
}
