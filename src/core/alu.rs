use primitive_types::{U256, U512};

#[inline(always)]
pub fn add(a: U256, b: U256) -> U256 {
    a.overflowing_add(b).0
}

#[inline(always)]
pub fn sub(a: U256, b: U256) -> U256 {
    a.overflowing_sub(b).0
}

#[inline(always)]
pub fn mul(a: U256, b: U256) -> U256 {
    a.overflowing_mul(b).0
}

#[inline(always)]
pub fn div(a: U256, b: U256) -> U256 {
    match a.checked_div(b) {
        Some(r) => r,
        None => U256::zero(),
    }
}

#[inline(always)]
pub fn neg(a: U256) -> U256 {
    a.overflowing_sub(U256::one()).0 ^ U256::MAX
}

#[inline(always)]
pub fn get_sign_abs(x: U256) -> (bool, U256) {
    if x.bit(255) {
        (true, neg(x))
    } else {
        (false, x)
    }
}

#[inline(always)]
pub fn sdiv(a: U256, b: U256) -> U256 {
    let (a_sign, a_abs) = get_sign_abs(a);
    let (b_sign, b_abs) = get_sign_abs(b);
    match a_abs.checked_div(b_abs) {
        Some(r) => {
            if a_sign ^ b_sign {
                neg(r)
            } else {
                r
            }
        }
        None => U256::zero(),
    }
}

#[inline(always)]
pub fn rem(a: U256, b: U256) -> U256 {
    match a.checked_rem(b) {
        Some(r) => r,
        None => U256::zero(),
    }
}

#[inline(always)]
pub fn smod(a: U256, b: U256) -> U256 {
    if b.is_zero() {
        return U256::zero()
    }
    let q = sdiv(a, b);
    a.overflowing_sub(b.overflowing_mul(q).0).0
}

#[test]
fn test_divisions() {
    let n = 1000;
    for i in -n..=n {
        for j in -n..=n {
            let a = if i < 0 {
                neg(((-i) as u32).into())
            } else {
                i.into()
            };
            let b = if j < 0 {
                neg(((-j) as u32).into())
            } else {
                j.into()
            };

            let t = sdiv(a, b);
            let t = if t.bit(255) {
                -(neg(t).as_u32() as i32)
            } else {
                t.as_u32() as i32
            };
            if j == 0 {
                assert_eq!(t, 0);
            } else {
                println!("{} / {} = {} (should be {})", i, j, t, i / j);
                assert_eq!(t, i / j);
            }

            let t = smod(a, b);
            let t = if t.bit(255) {
                -(neg(t).as_u32() as i32)
            } else {
                t.as_u32() as i32
            };
            if j == 0 {
                assert_eq!(t, 0);
            } else {
                println!("{} % {} = {} (should be {})", i, j, t, i % j);
                assert_eq!(t, i % j);
            }
        }
    }
}

#[inline(always)]
pub fn add_mod(a: U256, b: U256, n: U256) -> U256 {
    let a: U512 = a.into();
    let b: U512 = b.into();
    match (a + b).checked_rem(n.into()) {
        Some(r) => U256::try_from(r).unwrap(),
        None => U256::zero(),
    }
}

#[inline(always)]
pub fn mul_mod(a: U256, b: U256, n: U256) -> U256 {
    let a: U512 = a.into();
    let b: U512 = b.into();
    match (a * b).checked_rem(n.into()) {
        Some(r) => U256::try_from(r).unwrap(),
        None => U256::zero(),
    }
}

#[inline(always)]
pub fn exp(a: U256, b: U256) -> U256 {
    a.overflowing_pow(b).0
}

#[inline(always)]
pub fn sign_extend(mut back: U256, x: U256) -> U256 {
    let maxb: U256 = 31.into();
    if back > maxb {
        back = maxb
    }
    let shift: usize = (back.as_u32() as usize + 1) << 3;
    if x.bit(shift - 1) {
        // fill with 1s
        let mask: U256 = (U256::one() << (256 - shift)) - U256::one();
        x | (mask << shift)
    } else {
        x
    }
}

#[test]
fn test_sign_extend() {
    let mut xs = vec![0xaa, 0x55, 0xaaaa, 0x5555, 0xaaaaaaaau64, 0x55555555u64];
    for i in 0..1 << 20 {
        xs.push(i);
    }
    for x in xs.into_iter() {
        let mut nbits = 0;
        if x == 0 {
            nbits = 1
        } else {
            let mut y = x;
            while y > 0 {
                nbits += 1;
                y >>= 1;
            }
        }
        let back = (nbits + 7) / 8 - 1;
        let t = sign_extend(back.into(), x.into());
        if x >> ((back + 1) * 8 - 1) == 1 {
            assert_eq!(t ^ U256::MAX, ((!x) & ((1 << nbits) - 1)).into());
        } else {
            assert_eq!(t, x.into());
        }

        // another way is to check its property
        macro_rules! check {
            ($i: ident) => {
                let x = x as $i;
                if x < 0 {
                    assert_eq!(neg(t).as_u64(), -(x as i64) as u64);
                } else {
                    assert_eq!(t.as_u64(), x as u64);
                }
            };
        }
        if back == 0 {
            check!(i8);
        } else if back == 1 {
            check!(i16);
        } else if back == 2 {
            check!(i32);
        }
    }
}

#[inline(always)]
fn bool_to_u256(t: bool) -> U256 {
    if t {
        U256::one()
    } else {
        U256::zero()
    }
}

#[inline(always)]
pub fn lt(a: U256, b: U256) -> U256 {
    bool_to_u256(a < b)
}

#[inline(always)]
pub fn gt(a: U256, b: U256) -> U256 {
    bool_to_u256(a > b)
}

#[inline(always)]
pub fn slt(a: U256, b: U256) -> U256 {
    let (a_sign, a_abs) = get_sign_abs(a);
    let (b_sign, b_abs) = get_sign_abs(b);
    if a_sign ^ b_sign {
        // different signs
        bool_to_u256(a_sign)
    } else {
        // same signs
        bool_to_u256(if a_abs == b_abs {
            false
        } else {
            (a_abs < b_abs) ^ a_sign
        })
    }
}

#[inline(always)]
pub fn sgt(a: U256, b: U256) -> U256 {
    let (a_sign, a_abs) = get_sign_abs(a);
    let (b_sign, b_abs) = get_sign_abs(b);
    if a_sign ^ b_sign {
        bool_to_u256(b_sign)
    } else {
        bool_to_u256(if a_abs == b_abs {
            false
        } else {
            (a_abs > b_abs) ^ b_sign
        })
    }
}

#[test]
fn test_sign_cmp() {
    for i in -2..=2 {
        for j in -2..=2 {
            let slt1 = i < j;
            let slt2 = !slt(
                if i < 0 {
                    neg((-i as u64).into())
                } else {
                    i.into()
                },
                if j < 0 {
                    neg((-j as u64).into())
                } else {
                    j.into()
                },
            )
            .is_zero();
            println!("{} < {} = {} {}", i, j, slt1, slt2);
            assert_eq!(slt1, slt2);
            let sgt1 = i > j;
            let sgt2 = !sgt(
                if i < 0 {
                    neg((-i as u64).into())
                } else {
                    i.into()
                },
                if j < 0 {
                    neg((-j as u64).into())
                } else {
                    j.into()
                },
            )
            .is_zero();
            println!("{} > {} = {} {}", i, j, sgt1, sgt2);
            assert_eq!(sgt1, sgt2);
        }
    }
}

#[inline(always)]
pub fn eq(a: U256, b: U256) -> U256 {
    bool_to_u256(a == b)
}

#[inline(always)]
pub fn is_zero(a: U256) -> U256 {
    bool_to_u256(a.is_zero())
}

#[inline(always)]
pub fn and(a: U256, b: U256) -> U256 {
    a & b
}

#[inline(always)]
pub fn or(a: U256, b: U256) -> U256 {
    a | b
}

#[inline(always)]
pub fn xor(a: U256, b: U256) -> U256 {
    a ^ b
}

#[inline(always)]
pub fn not(a: U256) -> U256 {
    !a
}

#[inline(always)]
pub fn byte(i: U256, x: U256) -> U256 {
    let i = if i > 31.into() { 31 } else { i.as_u32() };
    (x >> (248 - (i << 3))) & 0xff.into()
}

#[inline(always)]
pub fn shl(s: U256, val: U256) -> U256 {
    if s < 256.into() {
        val << s
    } else {
        U256::zero()
    }
}

#[inline(always)]
pub fn shr(s: U256, val: U256) -> U256 {
    val >> s
}

#[inline(always)]
pub fn sar(s: U256, mut val: U256) -> U256 {
    let ext = val.bit(255);
    let s = if s > 256.into() {
        return if ext {
            // Max negative shift: all bits set
            U256::max_value()
        } else {
            U256::zero()
        }
    } else {
        s.as_u32()
    };
    val >>= s;
    if ext {
        // Sub with overflow handling as it can happen when shift 256 bits.
        val | (sub(U256::one() << s, 1.into())) << (256 - s)
    } else {
        val
    }
}

#[test]
fn test_sar() {
    // unsigned ints behave in the same way as shr
    for s in 0..255 {
        let x0 = shl(s.into(), 1.into());
        for i in 0..=256 {
            let x = shr(i.into(), x0);
            let nx = sar(i.into(), x0);
            assert_eq!(x, nx);
        }
    }
    for x in 2..1024 {
        for i in 0..=256 {
            let nx = neg(sar(i.into(), neg(x.into())));
            let ans = if i < 32 {
                // use Rust signed shift to test
                -((-x) >> i)
            } else if i <= 256 {
                // SAR(-1, 1) == -1
                1
            } else {
                // if the shift is greater than 256 the result is 0
                0
            };
            println!("{} {}", x, i);
            assert_eq!(nx, ans.into());
        }
    }
    assert_eq!(sar(256.into(), neg(1.into())), U256::max_value());
}
