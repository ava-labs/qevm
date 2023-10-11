pragma solidity ^0.8.0;

address constant myself = 0x3BDCAdc587be383B176C5f44402719CB4b7655A9;
  
contract Factorial {
    function fac(uint32 n) public returns(uint256){
        return n == 0 ? 1 : n * fac(n - 1);
    }
    function fac2(uint32 n) public returns(uint256){
        if (n == 0) {
            return 1;
        }
        bytes memory n2;
        bool b2;
        (b2, n2) = myself.call(abi.encodeWithSignature("fac2(uint32)", n - 1));
        return n * abi.decode(n2, (uint256));
    }
}
