//! Encodings checked against GNU as (`as --64 -msyntax=intel`).
use cera_syntax::asm::{assemble, assemble_raw};

const CASES: &[(&str, &str)] = &[
    ("mov rax, rbx", "4889d8"),
    ("mov eax, ebx", "89d8"),
    ("mov ax, bx", "6689d8"),
    ("mov al, bl", "88d8"),
    ("mov r8, r15", "4d89f8"),
    ("mov r8d, r9d", "4589c8"),
    ("mov sil, dil", "4088fe"),
    ("mov al, sil", "4088f0"),
    ("mov rax, 5", "48c7c005000000"),
    ("mov eax, 5", "b805000000"),
    ("mov ax, 5", "66b80500"),
    ("mov al, 5", "b005"),
    ("mov r9, 0x1122334455667788", "49b98877665544332211"),
    ("mov r10d, 7", "41ba07000000"),
    ("mov rax, -1", "48c7c0ffffffff"),
    ("mov qword ptr [rax], 5", "48c70005000000"),
    ("mov dword ptr [rbx+4], 7", "c7430407000000"),
    ("mov byte ptr [rsp], 1", "c6042401"),
    ("mov [rax], rbx", "488918"),
    ("mov rax, [rbx]", "488b03"),
    ("mov rax, [rbx+rcx*8]", "488b04cb"),
    ("mov rax, [rbx+rcx*8+0x10]", "488b44cb10"),
    ("mov rax, [rbp]", "488b4500"),
    ("mov rax, [r13]", "498b4500"),
    ("mov rax, [r12]", "498b0424"),
    ("mov rax, [rsp+8]", "488b442408"),
    ("mov rax, [rbp+0x1000]", "488b8500100000"),
    ("mov [r8+r9*4-8], r10", "4f895488f8"),
    ("mov rax, [0x1000]", "488b042500100000"),
    ("mov eax, [rax*4+0x10]", "8b048510000000"),
    ("add rax, rbx", "4801d8"),
    ("add rax, 5", "4883c005"),
    ("add rax, 0x1000", "480500100000"),
    ("add eax, 0x100", "0500010000"),
    ("add al, 5", "0405"),
    ("add ax, 0x1000", "66050010"),
    ("add qword ptr [rax], 1", "48830001"),
    ("add dword ptr [rax], 0x12345", "810045230100"),
    ("sub rsp, 8", "4883ec08"),
    ("and eax, 0xff", "25ff000000"),
    ("and rax, -16", "4883e0f0"),
    ("or r11, r12", "4d09e3"),
    ("xor eax, eax", "31c0"),
    ("xor r8d, r8d", "4531c0"),
    ("cmp rax, rbx", "4839d8"),
    ("cmp byte ptr [rax], 0", "803800"),
    ("cmp rcx, 0x7fffffff", "4881f9ffffff7f"),
    ("adc rax, rdx", "4811d0"),
    ("sbb ecx, 1", "83d901"),
    ("lea rax, [rbx+rcx*2+5]", "488d444b05"),
    ("lea r8, [rsp+16]", "4c8d442410"),
    ("movzx eax, bl", "0fb6c3"),
    ("movzx eax, byte ptr [rbx]", "0fb603"),
    ("movzx rax, word ptr [rbx]", "480fb703"),
    ("movsx rax, bl", "480fbec3"),
    ("movsx eax, word ptr [rbx]", "0fbf03"),
    ("movsxd rax, ecx", "4863c1"),
    ("movsxd rax, dword ptr [rbx]", "486303"),
    ("test rax, rax", "4885c0"),
    ("test al, 1", "a801"),
    ("test eax, 0x100", "a900010000"),
    ("test dword ptr [rax], 4", "f70004000000"),
    ("test cl, dl", "84d1"),
    ("inc rax", "48ffc0"),
    ("dec ecx", "ffc9"),
    ("inc qword ptr [rax]", "48ff00"),
    ("neg r9", "49f7d9"),
    ("not eax", "f7d0"),
    ("mul rcx", "48f7e1"),
    ("div r8", "49f7f0"),
    ("idiv dword ptr [rbx]", "f73b"),
    ("imul rax, rbx", "480fafc3"),
    ("imul rax, rbx, 10", "486bc30a"),
    ("imul eax, [rbx], 1000", "6903e8030000"),
    ("imul rcx", "48f7e9"),
    ("shl rax, 1", "48d1e0"),
    ("shl rax, 4", "48c1e004"),
    ("shr eax, cl", "d3e8"),
    ("sar r8, 3", "49c1f803"),
    ("rol ax, 2", "66c1c002"),
    ("ror byte ptr [rax], 1", "d008"),
    ("push rax", "50"),
    ("push r12", "4154"),
    ("pop r15", "415f"),
    ("push 5", "6a05"),
    ("push 0x1000", "6800100000"),
    ("push qword ptr [rax]", "ff30"),
    ("pop qword ptr [rbx]", "8f03"),
    ("xchg rax, rbx", "4893"),
    ("xchg rbx, rax", "4893"),
    ("xchg rcx, rdx", "4887d1"),
    ("xchg [rax], rbx", "488718"),
    ("bswap rax", "480fc8"),
    ("bswap r9d", "410fc9"),
    ("cmovne rax, rbx", "480f45c3"),
    ("cmove ecx, [rax]", "0f4408"),
    ("setz al", "0f94c0"),
    ("setne r8b", "410f95c0"),
    ("sete byte ptr [rax]", "0f9400"),
    ("bt rax, rbx", "480fa3d8"),
    ("bts rax, 5", "480fbae805"),
    ("btr dword ptr [rax], 3", "0fba3003"),
    ("bsf rax, rbx", "480fbcc3"),
    ("bsr ecx, edx", "0fbdca"),
    ("popcnt rax, rbx", "f3480fb8c3"),
    ("lzcnt eax, ecx", "f30fbdc1"),
    ("tzcnt r8, r9", "f34d0fbcc1"),
    ("xadd [rax], rbx", "480fc118"),
    ("cmpxchg [rcx], rdx", "480fb111"),
    ("lock xadd [rax], ebx", "f00fc118"),
    ("lock cmpxchg [rcx], edx", "f00fb111"),
    ("rep movsb", "f3a4"),
    ("rep stosq", "f348ab"),
    ("repne scasb", "f2ae"),
    ("syscall", "0f05"),
    ("cpuid", "0fa2"),
    ("rdtsc", "0f31"),
    ("rdmsr", "0f32"),
    ("wrmsr", "0f30"),
    ("cli", "fa"),
    ("sti", "fb"),
    ("hlt", "f4"),
    ("pause", "f390"),
    ("nop", "90"),
    ("int 0x80", "cd80"),
    ("int3", "cc"),
    ("in al, dx", "ec"),
    ("in eax, 0x60", "e560"),
    ("out dx, al", "ee"),
    ("out 0x60, al", "e660"),
    ("out dx, eax", "ef"),
    ("invlpg [rax]", "0f0138"),
    ("lgdt [rax]", "0f0110"),
    ("lidt [rbx+8]", "0f015b08"),
    ("ltr ax", "0f00d8"),
    ("mov cr3, rax", "0f22d8"),
    ("mov rax, cr0", "0f20c0"),
    ("mov cr4, r9", "410f22e1"),
    ("iretq", "48cf"),
    ("swapgs", "0f01f8"),
    ("pushfq", "9c"),
    ("popfq", "9d"),
    ("cqo", "4899"),
    ("cdqe", "4898"),
    ("cdq", "99"),
    ("mfence", "0faef0"),
    ("lfence", "0faee8"),
    ("call rax", "ffd0"),
    ("jmp rbx", "ffe3"),
    ("call [rax+8]", "ff5008"),
    ("jmp qword ptr [rip+0x10]", "ff2510000000"),
    ("mov rax, [rip+0x10]", "488b0510000000"),
    (".byte 1, 2, 3", "010203"),
    (".quad 0x1122334455667788", "8877665544332211"),
];

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn encodings_match_gnu_as() {
    for (src, want) in CASES {
        assert_eq!(hex(&assemble_raw(src).unwrap()), *want, "{src}");
    }
}

#[test]
fn labels_resolve_both_directions() {
    // jmp +5 over a 5-byte mov; a backward jnz.
    let b = assemble_raw("jmp skip\nmov eax, 1\nskip: nop\nback: dec ecx\njnz back").unwrap();
    assert_eq!(hex(&b), "e905000000b8010000009" .to_string() + "0ffc90f85f8ffffff");
}

#[test]
fn errors_name_the_line() {
    let e = assemble("nop\nfrobnicate rax").unwrap_err();
    assert!(e.contains("line 2") && e.contains("frobnicate"), "{e}");
    assert!(assemble("jmp nowhere").unwrap_err().contains("unknown label"));
    assert!(assemble("a: nop\na: nop").unwrap_err().contains("twice"));
    assert!(assemble("mov [rax], 5").unwrap_err().contains("size"));
    assert!(assemble("mov eax, 0x1ffffffff").is_err());
}

#[test]
fn wrapped_block_is_a_complete_function() {
    let b = assemble("mov rax, rdi").unwrap();
    assert_eq!(b.last(), Some(&0xC3));
    assert_eq!(&b[..2], &[0x53, 0x55]);
}
