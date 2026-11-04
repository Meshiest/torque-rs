mod common;

use common::{empty, encode, t3d};
use torque_ts_bytecode::decode::{DecodeError, decode_dso};
use torque_ts_bytecode::opcodes::{IDENT_WORDS, Op, OpcodeTable, OperandShape as S, Tail};
use torque_ts_bytecode::{Dialect, DsoFile};

#[test]
fn the_operand_table_matches_the_documented_layouts() {
    assert_eq!(
        Op::FuncDecl.operands(),
        &[
            S::Ident,
            S::Ident,
            S::Ident,
            S::BodyAndLine,
            S::Target,
            S::Count,
            S::Immediate
        ]
    );
    assert_eq!(Op::FuncDecl.tail(), Tail::Arguments);
    assert_eq!(
        Op::CreateObject.operands(),
        &[S::Ident, S::Flag, S::Flag, S::Flag, S::Immediate, S::Target]
    );
    for op in [Op::AddObject, Op::EndObject, Op::SetCurObjectInternal] {
        assert_eq!(op.operands(), &[S::Flag], "{}", op.name());
    }
    for op in [Op::IterBegin, Op::IterBeginStr] {
        assert_eq!(op.operands(), &[S::Flag]);
        assert_eq!(op.tail(), Tail::IterVariable);
    }
    for op in [
        Op::Jmp,
        Op::JmpIf,
        Op::JmpIfNot,
        Op::JmpNotString,
        Op::JmpIfF,
        Op::JmpIfFNot,
        Op::JmpIfNp,
        Op::JmpIfNotNp,
        Op::Iter,
    ] {
        assert_eq!(op.operands(), &[S::Target], "{}", op.name());
    }
    for op in [
        Op::SetCurVar,
        Op::SetCurVarCreate,
        Op::SetCurField,
        Op::LoadImmedIdent,
    ] {
        assert_eq!(op.operands(), &[S::Ident], "{}", op.name());
    }
    for op in [
        Op::Inc,
        Op::LoadLocalVarUint,
        Op::LoadLocalVarFlt,
        Op::LoadLocalVarStr,
        Op::SaveLocalVarUint,
        Op::SaveLocalVarFlt,
        Op::SaveLocalVarStr,
    ] {
        assert_eq!(op.operands(), &[S::Register], "{}", op.name());
    }
    for op in [Op::LoadImmedUint, Op::SetCurFieldType, Op::PushFrame] {
        assert_eq!(op.operands(), &[S::Immediate], "{}", op.name());
    }
    assert_eq!(Op::LoadImmedFlt.operands(), &[S::FloatIndex]);
    for op in [Op::LoadImmedStr, Op::TagToStr, Op::DocblockStr, Op::Assert] {
        assert_eq!(op.operands(), &[S::StringOffset], "{}", op.name());
    }
    assert_eq!(Op::CallFunc.operands(), &[S::Ident, S::Ident, S::Immediate]);
    assert_eq!(Op::AdvanceStrAppendChar.operands(), &[S::Char]);
    // Everything else takes no operands; only FuncDecl and the two iterator starts have tails.
    let fixed: usize = Op::ALL
        .iter()
        .filter(|op| !op.operands().is_empty())
        .count();
    assert_eq!(fixed, 37);
    for op in Op::ALL {
        let tailed = matches!(op, Op::FuncDecl | Op::IterBegin | Op::IterBeginStr);
        assert_eq!(op.tail() != Tail::None, tailed, "{}", op.name());
    }
    assert_eq!(S::Ident.words(), IDENT_WORDS);
    assert_eq!(IDENT_WORDS, 2);
    assert_eq!(S::Register.words(), 1);
}

#[test]
fn op_index_and_count_agree_with_all() {
    assert_eq!(Op::COUNT, 90);
    for (i, op) in Op::ALL.iter().enumerate() {
        assert_eq!(op.index(), i);
    }
}

#[test]
fn from_codes_rejects_duplicates_and_accepts_a_permutation() {
    let mut codes = [0u32; Op::COUNT];
    for (i, c) in codes.iter_mut().enumerate() {
        *c = 100 + i as u32;
    }
    assert!(OpcodeTable::from_codes(codes).is_ok());
    codes[5] = codes[4];
    assert_eq!(
        OpcodeTable::from_codes(codes),
        Err(torque_ts_bytecode::opcodes::DuplicateCode(104))
    );
}

fn dso_with_code(code: Vec<u32>) -> DsoFile {
    let mut s = empty();
    s.global_strings = b"\0".to_vec();
    s.code = code;
    DsoFile::parse(&encode(&s), &t3d()).unwrap()
}

#[test]
fn a_remapped_numbering_decodes_the_same_program() {
    let mut codes = [0u32; Op::COUNT];
    for (i, c) in codes.iter_mut().enumerate() {
        *c = 100 + i as u32; // all codes now >= 100, so none collides with Torque3D's
    }
    let remapped_dialect = Dialect {
        opcodes: OpcodeTable::from_codes(codes).unwrap(),
        ..Dialect::torque3d()
    };
    let table = &remapped_dialect.opcodes;
    let ours = t3d().opcodes;
    let built_in = dso_with_code(vec![ours.code(Op::LoadImmedUint), 5, ours.code(Op::Return)]);
    let remapped = dso_with_code(vec![
        table.code(Op::LoadImmedUint),
        5,
        table.code(Op::Return),
    ]);
    let a = decode_dso(&built_in, &t3d()).unwrap();
    let b = decode_dso(&remapped, &remapped_dialect).unwrap();
    assert_eq!(a, b);
    // Torque3D's numbering does not understand the remapped program.
    assert!(matches!(
        decode_dso(&remapped, &t3d()),
        Err(DecodeError::InvalidOpcode { .. })
    ));
}
