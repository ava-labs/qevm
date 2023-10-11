#![cfg(feature = "tx")]

use qevm::tx::*;

#[test]
fn test_encode() {
    let chain_id: qevm::common::U256 = (0xa86au64).into();
    let bytes0 = hex::decode("02f89482a86a822ff8843b9aca008517d0200bea83028d019482a85407bd612f52577909f4a58bfc6873f14da880a42d6ef31000000000000000000000000000000000000000000000000000000000002c6d13c001a08a2225cf1cbc17447916c615a67683b02d118f945f9e37202ce83a83887bd973a071dc4ce1ddd0138107a5eb70ddbabf2f4bef74f836e7203db1cc4fad9abe52c7").unwrap();
    let tx0 = Tx::decode(&bytes0, &chain_id).unwrap();
    let enc0 = tx0.encode();
    assert_eq!(bytes0, enc0);
    assert_eq!(enc0, Tx::decode(&enc0, &chain_id).unwrap().encode());
    assert!(tx0.type_() == TxType::DynamicFee);
    assert!(tx0.to().is_some());

    let bytes1 = hex::decode("f86f808534630b8a0082520894503560430e4b5814dda09ac789c3508bb41b24b28814d1120d7b16000080830150f7a0f43b1f16ac865db9fb2ee37d55634b2a0f16b16dcd35fcefe34a6fb6c8b67c8aa0786add8000604cb78401187d9e53cd073fb8cd78474639c9d35a8b814de4fcff").unwrap();
    let tx1 = Tx::decode(&bytes1, &chain_id).unwrap();
    let enc1 = tx1.encode();
    assert_eq!(bytes1, enc1);
    assert_eq!(enc1, Tx::decode(&enc1, &chain_id).unwrap().encode());
    assert!(tx1.type_() == TxType::Legacy);

    let bytes2 = hex::decode("f9091f04851176592e01835b8d808080b908c9608060405234801561001057600080fd5b506040516108a93803806108a98339818101604052602081101561003357600080fd5b5051808061004081610051565b5061004a336100c3565b5050610123565b610064816100e760201b61042a1760201c565b61009f5760405162461bcd60e51b815260040180806020018281038252603b81526020018061086e603b913960400191505060405180910390fd5b7f7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c355565b7f10d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b55565b6000813f7fc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a47081811480159061011b57508115155b949350505050565b61073c806101326000396000f3fe60806040526004361061005a5760003560e01c80635c60da1b116100435780635c60da1b146101315780638f2839701461016f578063f851a440146101af5761005a565b80633659cfe6146100645780634f1ef286146100a4575b6100626101c4565b005b34801561007057600080fd5b506100626004803603602081101561008757600080fd5b503573ffffffffffffffffffffffffffffffffffffffff166101de565b610062600480360360408110156100ba57600080fd5b73ffffffffffffffffffffffffffffffffffffffff82351691908101906040810160208201356401000000008111156100f257600080fd5b82018360208201111561010457600080fd5b8035906020019184600183028401116401000000008311171561012657600080fd5b509092509050610232565b34801561013d57600080fd5b50610146610309565b6040805173ffffffffffffffffffffffffffffffffffffffff9092168252519081900360200190f35b34801561017b57600080fd5b506100626004803603602081101561019257600080fd5b503573ffffffffffffffffffffffffffffffffffffffff16610318565b3480156101bb57600080fd5b50610146610420565b6101cc610466565b6101dc6101d76104fa565b61051f565b565b6101e6610543565b73ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff1614156102275761022281610568565b61022f565b61022f6101c4565b50565b61023a610543565b73ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff1614156102fc5761027683610568565b60003073ffffffffffffffffffffffffffffffffffffffff16348484604051808383808284376040519201945060009350909150508083038185875af1925050503d80600081146102e3576040519150601f19603f3d011682016040523d82523d6000602084013e6102e8565b606091505b50509050806102f657600080fd5b50610304565b6103046101c4565b505050565b60006103136104fa565b905090565b610320610543565b73ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff1614156102275773ffffffffffffffffffffffffffffffffffffffff81166103bf576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004018080602001828103825260368152602001806106966036913960400191505060405180910390fd5b7f7e644d79422f17c01e4894b5f4f588d331ebfa28653d42ae832dc59e38c9798f6103e8610543565b6040805173ffffffffffffffffffffffffffffffffffffffff928316815291841660208301528051918290030190a1610222816105bd565b6000610313610543565b6000813f7fc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a47081811480159061045e57508115155b949350505050565b61046e610543565b73ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff1614156104f2576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004018080602001828103825260328152602001806106646032913960400191505060405180910390fd5b6101dc6101dc565b7f7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c35490565b3660008037600080366000845af43d6000803e80801561053e573d6000f35b3d6000fd5b7f10d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b5490565b610571816105e1565b6040805173ffffffffffffffffffffffffffffffffffffffff8316815290517fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b9181900360200190a150565b7f10d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b55565b6105ea8161042a565b61063f576040517f08c379a000000000000000000000000000000000000000000000000000000000815260040180806020018281038252603b8152602001806106cc603b913960400191505060405180910390fd5b7f7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c35556fe43616e6e6f742063616c6c2066616c6c6261636b2066756e6374696f6e2066726f6d207468652070726f78792061646d696e43616e6e6f74206368616e6765207468652061646d696e206f6620612070726f787920746f20746865207a65726f206164647265737343616e6e6f742073657420612070726f787920696d706c656d656e746174696f6e20746f2061206e6f6e2d636f6e74726163742061646472657373a26469706673582212206715e283f350a976c05fe8b17fc01929cb137086f941d36864b48c4269d883b264736f6c634300060c003343616e6e6f742073657420612070726f787920696d706c656d656e746174696f6e20746f2061206e6f6e2d636f6e747261637420616464726573730000000000000000000000007f2239511051b875ccf84dab02d5a307adcd51c2830150f7a0ca0e339d792802c82b6b174826f42e5069603f803a299fce36bee0cd44283483a03a8b993fc89c5663298ac5341ae61d6b7cefa1f68dbaf13e3d36ee1a84cd8098").unwrap();
    let tx2 = Tx::decode(&bytes2, &chain_id).unwrap();
    let enc2 = tx2.encode();
    assert_eq!(bytes2, enc2);
    assert_eq!(enc2, Tx::decode(&enc2, &chain_id).unwrap().encode());
    assert!(tx2.type_() == TxType::Legacy);
    assert!(tx2.to().is_none());
}