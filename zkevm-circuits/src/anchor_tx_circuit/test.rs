#![allow(unused_imports)]
use std::collections::HashMap;

use super::{
    sign_verify::{GOLDEN_TOUCH_ADDRESS, GOLDEN_TOUCH_PRIVATEKEY, GOLDEN_TOUCH_WALLET, GX1, GX2},
    *,
};
use crate::{
    util::{log2_ceil, unusable_rows},
    witness::{block_convert, Block},
};
use bus_mapping::{
    circuit_input_builder::{CircuitInputBuilder, CircuitsParams},
    mock::BlockData,
};
use eth_types::{
    address, bytecode,
    geth_types::{GethData, Transaction},
    sign_types::{biguint_to_32bytes_le, ct_option_ok_or, sign, SignData, SECP256K1_Q},
    word, Address, Field, ToBigEndian, ToLittleEndian, ToWord, Word, U256,
};
use ethers_core::types::TransactionRequest;
use ethers_signers::{LocalWallet, Signer};
use gadgets::{
    is_equal::IsEqualChip,
    mul_add::{MulAddChip, MulAddConfig},
    util::{split_u256, Expr},
};
use halo2_proofs::{
    arithmetic::Field as _,
    circuit::{Layouter, Region, Value},
    dev::{MockProver, VerifyFailure},
    halo2curves::{
        bn256::Fr,
        ff::PrimeField,
        group::Curve,
        secp256k1::{self, Secp256k1Affine},
    },
    plonk::{Advice, Column, ConstraintSystem, Error, Expression, Fixed, SecondPhase, Selector},
    poly::Rotation,
};
use itertools::Itertools;
use log::error;
use mock::{AddrOrWallet, TestContext, MOCK_CHAIN_ID};
use num::Integer;
use num_bigint::BigUint;
use once_cell::sync::Lazy;
use sha3::{Digest, Keccak256};

#[test]
fn tx_circuit_unusable_rows() {
    assert_eq!(
        AnchorTxCircuit::<Fr>::unusable_rows(),
        unusable_rows::<Fr, TestAnchorTxCircuit::<Fr>>(()),
    )
}

pub(crate) fn anchor_sign(
    anchor_tx: &Transaction,
    chain_id: u64,
) -> Result<SignData, eth_types::Error> {
    // msg = rlp([nonce, gasPrice, gas, to, value, data, sig_v, r, s])
    let req: TransactionRequest = anchor_tx.into();
    let msg = req.chain_id(chain_id).rlp();
    let msg_hash: [u8; 32] = Keccak256::digest(&msg)
        .as_slice()
        .to_vec()
        .try_into()
        .expect("hash length isn't 32 bytes");
    // msg_hash = msg_hash % q
    let msg_hash = BigUint::from_bytes_be(msg_hash.as_slice());
    let msg_hash = msg_hash.mod_floor(&*SECP256K1_Q);
    let msg_hash_le = biguint_to_32bytes_le(msg_hash);
    let msg_hash = ct_option_ok_or(
        secp256k1::Fq::from_repr(msg_hash_le),
        libsecp256k1::Error::InvalidMessage,
    )?;
    let k1 = secp256k1::Fq::ONE;
    let sk = ct_option_ok_or(
        secp256k1::Fq::from_repr(GOLDEN_TOUCH_PRIVATEKEY.to_le_bytes()),
        libsecp256k1::Error::InvalidSecretKey,
    )?;
    let generator = Secp256k1Affine::generator();
    let pk = generator * sk;
    let pk = pk.to_affine();
    let (mut sig_r, mut sig_s) = sign(k1, sk, msg_hash);
    let gx1 = ct_option_ok_or(
        secp256k1::Fq::from_repr(GX1.to_le_bytes()),
        libsecp256k1::Error::InvalidSignature,
    )?;
    assert!(sig_r == gx1);
    if sig_s == secp256k1::Fq::ZERO {
        let k2 = secp256k1::Fq::ONE + secp256k1::Fq::ONE;
        (sig_r, sig_s) = sign(k2, sk, msg_hash);
        let gx2 = ct_option_ok_or(
            secp256k1::Fq::from_repr(GX2.to_le_bytes()),
            libsecp256k1::Error::InvalidSignature,
        )?;
        assert!(sig_r == gx2);
    }
    Ok(SignData {
        signature: (sig_r, sig_s),
        pk,
        msg_hash,
    })
}

fn run<F: Field>(block: &Block<F>) -> Result<(), Vec<VerifyFailure>> {
    let k = log2_ceil(
        AnchorTxCircuit::<Fr>::unusable_rows()
            + AnchorTxCircuit::<Fr>::min_num_rows(block.circuits_params.max_txs),
    );
    let circuit = TestAnchorTxCircuit::<F>::new_from_block(block);

    let prover = match MockProver::run(k + 1, &circuit, vec![]) {
        Ok(prover) => prover,
        Err(e) => panic!("{:#?}", e),
    };
    prover.verify()
}

fn gen_block<const NUM_TXS: usize>(max_txs: usize, max_calldata: usize, taiko: Taiko) -> Block<Fr> {
    let chain_id = (*MOCK_CHAIN_ID).as_u64();
    let mut wallets = HashMap::new();
    wallets.insert(
        GOLDEN_TOUCH_ADDRESS.clone(),
        GOLDEN_TOUCH_WALLET.clone().with_chain_id(chain_id),
    );

    let code = bytecode! {
        PUSH1(0x01) // value
        PUSH1(0x02) // key
        SSTORE

        PUSH3(0xbb)
    };

    let block: GethData = TestContext::<2, NUM_TXS>::new(
        None,
        |accs| {
            accs[0]
                .address(GOLDEN_TOUCH_ADDRESS.clone())
                .balance(Word::from(1u64 << 20));
            accs[1].address(taiko.l2_contract).code(code);
        },
        |mut txs, _accs| {
            txs[0]
                .gas(taiko.anchor_gas_cost.to_word())
                .gas_price(ANCHOR_TX_GAS_PRICE.to_word())
                .from(GOLDEN_TOUCH_ADDRESS.clone())
                .to(taiko.l2_contract)
                .input(taiko.anchor_call())
                .nonce(0)
                .value(ANCHOR_TX_VALUE.to_word());
            let tx: Transaction = txs[0].to_owned().into();
            let sig_data = anchor_sign(&tx, chain_id).unwrap();
            let sig_r = U256::from_little_endian(sig_data.signature.0.to_bytes().as_slice());
            let sig_s = U256::from_little_endian(sig_data.signature.1.to_bytes().as_slice());
            txs[0].sig_data((2712, sig_r, sig_s));
        },
        |block, _tx| block,
    )
    .unwrap()
    .into();
    let circuits_params = CircuitsParams {
        max_txs,
        max_calldata,
        ..Default::default()
    };
    let mut builder = BlockData::new_from_geth_data_with_params(block.clone(), circuits_params)
        .new_circuit_input_builder();
    builder
        .handle_block(&block.eth_block, &block.geth_traces)
        .unwrap();
    let mut block = block_convert::<Fr>(&builder.block, &builder.code_db).unwrap();
    block.taiko = taiko;
    block
}

#[test]
fn test() {
    let mut taiko = Taiko::default();
    taiko.anchor_gas_cost = 150000;
    let block = gen_block::<1>(2, 100, taiko);
    assert_eq!(run::<Fr>(&block), Ok(()));
}
