pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mantissa = (h & 0x3FF) as u32;

    if exp == 0 {
        if mantissa == 0 {
            f32::from_bits(sign << 31)
        } else {
            // Subnormal
            let mut m = mantissa;
            let mut e = 0;
            while (m & 0x400) == 0 {
                m <<= 1;
                e += 1;
            }
            let new_exp = 127 - 14 - e;
            let new_mantissa = (m & 0x3FF) << 13;
            f32::from_bits((sign << 31) | (new_exp << 23) | new_mantissa)
        }
    } else if exp == 31 {
        // Inf or NaN
        f32::from_bits((sign << 31) | 0x7F800000 | (mantissa << 13))
    } else {
        // Normal
        let new_exp = exp + 127 - 15;
        let new_mantissa = mantissa << 13;
        f32::from_bits((sign << 31) | (new_exp << 23) | new_mantissa)
    }
}

pub fn f32_to_f16(val: f32) -> u16 {
    let bits = val.to_bits();
    let sign = ((bits >> 31) & 1) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x7FFFFF;

    if exp == 255 {
        (sign << 15) | (0x1F << 10) | ((mantissa >> 13) as u16)
    } else {
        let new_exp = exp - 127 + 15;
        if new_exp >= 31 {
            (sign << 15) | (0x1F << 10)
        } else if new_exp <= 0 {
            sign << 15
        } else {
            (sign << 15) | ((new_exp as u16) << 10) | ((mantissa >> 13) as u16)
        }
    }
}
