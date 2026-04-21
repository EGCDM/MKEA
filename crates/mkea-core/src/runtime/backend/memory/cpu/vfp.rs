impl MemoryArm32Backend {
// VFP execution helpers and literal/multi-transfer handling.

    fn try_exec_exact_vfp_opcode_override(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let word28 = word & 0x0FFF_FFFF;
        match word28 {
            0x0d9f8a57 => {
                let base = current_pc.wrapping_add(8);
                let addr = base.wrapping_add(0x57 * 4);
                let start_reg = 16u32;
                let end_reg = 16u32;
                let count = 1u32;
                let precision = "single";
                let decoder_branch = "exact-arm-vldr-literal-override-cond";
                let reason = "0x0d9f8a57 must decode as conditional literal single-transfer vldr s16, [pc, #348], not as VFP multi-register load";

                let value = match self.read_u32_le(addr) {
                    Ok(value) => value,
                    Err(err) => return Some(Err(err)),
                };
                self.vfp_set_s(start_reg as usize, value);

                self.exec.vfp_exact_opcode_hits = self.exec.vfp_exact_opcode_hits.saturating_add(1);
                self.exec.vfp_exact_override_hits = self.exec.vfp_exact_override_hits.saturating_add(1);
                self.exec.vfp_single_transfer_ops = self.exec.vfp_single_transfer_ops.saturating_add(1);
                self.exec.vfp_pc_base_ops = self.exec.vfp_pc_base_ops.saturating_add(1);
                self.exec.vfp_pc_base_load_ops = self.exec.vfp_pc_base_load_ops.saturating_add(1);
                self.exec.vfp_last_pc_base_addr = Some(addr);
                self.exec.vfp_last_pc_base_word = Some(word);
                self.exec.vfp_last_transfer_addr = Some(addr);
                self.exec.vfp_last_transfer_start_reg = Some(start_reg);
                self.exec.vfp_last_transfer_end_reg = Some(end_reg);
                self.exec.vfp_last_transfer_count = Some(count);
                self.exec.vfp_last_transfer_precision = Some(precision.to_string());
                self.exec.vfp_last_transfer_mode = Some("literal-load".to_string());
                self.exec.vfp_last_exact_decoder_branch = Some(decoder_branch.to_string());
                self.exec.vfp_last_exact_reason = Some(reason.to_string());
                self.exec.vfp_last_exact_opcode = Some(format!(
                    "word=0x{word:08x} lower28=0x{word28:08x} branch={} base=pc@0x{base:08x} addr=0x{addr:08x} start={} end={} count={} precision={}",
                    decoder_branch,
                    start_reg,
                    end_reg,
                    count,
                    precision,
                ));
                let detail = format!(
                    "vldr {{s{}}} [pc@0x{:08x}, #+348] addr=0x{:08x} precision={} via {}",
                    start_reg,
                    base,
                    addr,
                    precision,
                    decoder_branch,
                );
                self.exec.vfp_last_op = Some(detail.clone());
                self.diag.trace.push(format!("     ↳ {}", detail));
                self.cpu.regs[15] = current_pc.wrapping_add(4);
                Some(Ok(StepControl::Continue))
            }
            0x0d9f6a2c => {
                let base = current_pc.wrapping_add(8);
                let addr = base.wrapping_add(0x2c * 4);
                let start_reg = 12u32;
                let end_reg = 12u32;
                let count = 1u32;
                let precision = "single";
                let decoder_branch = "exact-arm-vldr-literal-override";
                let reason = "0xed9f6a2c must decode as literal single-transfer vldr s12, [pc, #176], not as multi-register vldm/vldmia range";

                let value = match self.read_u32_le(addr) {
                    Ok(value) => value,
                    Err(err) => return Some(Err(err)),
                };
                self.vfp_set_s(start_reg as usize, value);

                self.exec.vfp_exact_opcode_hits = self.exec.vfp_exact_opcode_hits.saturating_add(1);
                self.exec.vfp_exact_override_hits = self.exec.vfp_exact_override_hits.saturating_add(1);
                self.exec.vfp_single_transfer_ops = self.exec.vfp_single_transfer_ops.saturating_add(1);
                self.exec.vfp_pc_base_ops = self.exec.vfp_pc_base_ops.saturating_add(1);
                self.exec.vfp_pc_base_load_ops = self.exec.vfp_pc_base_load_ops.saturating_add(1);
                self.exec.vfp_last_pc_base_addr = Some(addr);
                self.exec.vfp_last_pc_base_word = Some(word);
                self.exec.vfp_last_transfer_addr = Some(addr);
                self.exec.vfp_last_transfer_start_reg = Some(start_reg);
                self.exec.vfp_last_transfer_end_reg = Some(end_reg);
                self.exec.vfp_last_transfer_count = Some(count);
                self.exec.vfp_last_transfer_precision = Some(precision.to_string());
                self.exec.vfp_last_transfer_mode = Some("literal-load".to_string());
                self.exec.vfp_last_exact_decoder_branch = Some(decoder_branch.to_string());
                self.exec.vfp_last_exact_reason = Some(reason.to_string());
                self.exec.vfp_last_exact_opcode = Some(format!(
                    "word=0x{word:08x} branch={} base=pc@0x{base:08x} addr=0x{addr:08x} start={} end={} count={} precision={}",
                    decoder_branch,
                    start_reg,
                    end_reg,
                    count,
                    precision,
                ));
                let detail = format!(
                    "vldr {{s{}}} [pc@0x{:08x}, #+176] addr=0x{:08x} precision={} via {}",
                    start_reg,
                    base,
                    addr,
                    precision,
                    decoder_branch,
                );
                self.exec.vfp_last_op = Some(detail.clone());
                self.diag.trace.push(format!("     ↳ {}", detail));
                self.cpu.regs[15] = current_pc.wrapping_add(4);
                Some(Ok(StepControl::Continue))
            }
            0x0dd37a00 => {
                let base_reg = 3usize;
                let base = self.reg_operand(base_reg, current_pc);
                let addr = base;
                let start_reg = 15u32;
                let end_reg = 15u32;
                let count = 1u32;
                let precision = "single";
                let decoder_branch = "exact-arm-vldr-r3-override";
                let reason = "0xedd37a00 must decode as single-transfer vldr s15, [r3] with a single register, not as VFP multi-register list";

                let value = match self.read_u32_le(addr) {
                    Ok(value) => value,
                    Err(err) => return Some(Err(err)),
                };
                self.vfp_set_s(start_reg as usize, value);

                self.exec.vfp_exact_opcode_hits = self.exec.vfp_exact_opcode_hits.saturating_add(1);
                self.exec.vfp_exact_override_hits = self.exec.vfp_exact_override_hits.saturating_add(1);
                self.exec.vfp_single_transfer_ops = self.exec.vfp_single_transfer_ops.saturating_add(1);
                self.exec.vfp_last_transfer_addr = Some(addr);
                self.exec.vfp_last_transfer_start_reg = Some(start_reg);
                self.exec.vfp_last_transfer_end_reg = Some(end_reg);
                self.exec.vfp_last_transfer_count = Some(count);
                self.exec.vfp_last_transfer_precision = Some(precision.to_string());
                self.exec.vfp_last_transfer_mode = Some("base-load".to_string());
                self.exec.vfp_last_exact_decoder_branch = Some(decoder_branch.to_string());
                self.exec.vfp_last_exact_reason = Some(reason.to_string());
                self.exec.vfp_last_exact_opcode = Some(format!(
                    "word=0x{word:08x} branch={} base=r{}@0x{base:08x} addr=0x{addr:08x} start={} end={} count={} precision={}",
                    decoder_branch,
                    base_reg,
                    start_reg,
                    end_reg,
                    count,
                    precision,
                ));
                let detail = format!(
                    "vldr {{s{}}} [r{}@0x{:08x}] addr=0x{:08x} precision={} via {}",
                    start_reg,
                    base_reg,
                    base,
                    addr,
                    precision,
                    decoder_branch,
                );
                self.exec.vfp_last_op = Some(detail.clone());
                self.diag.trace.push(format!("     ↳ {}", detail));
                self.cpu.regs[15] = current_pc.wrapping_add(4);
                Some(Ok(StepControl::Continue))
            }
            _ => None,
        }
    }

    fn is_vfp_literal_single_transfer(word: u32) -> bool {
        let masked = word & 0x0F30_0E00;
        masked == 0x0D10_0A00 || masked == 0x0D00_0A00
    }

    fn try_exec_vfp_literal_single_transfer(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        if !Self::is_vfp_literal_single_transfer(word) {
            return None;
        }

        let p = ((word >> 24) & 1) != 0;
        let u = ((word >> 23) & 1) != 0;
        let d_bit = ((word >> 22) & 1) != 0;
        let w = ((word >> 21) & 1) != 0;
        let l = ((word >> 20) & 1) != 0;
        let rn = ((word >> 16) & 0xF) as usize;
        let vd = ((word >> 12) & 0xF) as usize;
        let double_precision = ((word >> 8) & 1) != 0;
        let imm8 = (word & 0xFF) as u32;

        if !p || w {
            return Some(Err(CoreError::Unsupported(format!(
                "VFP single-transfer with unsupported addressing mode is not implemented yet for 0x{word:08x}"
            ))));
        }

        let base = self.reg_operand(rn, current_pc);
        let offset = imm8.wrapping_mul(4);
        let addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let pc_base = rn == 15;

        let (start_reg, end_reg, reg_text, precision) = if double_precision {
            let reg = (((d_bit as usize) << 4) | vd) as usize;
            if reg >= self.exec.vfp_d_regs.len() {
                return Some(Err(CoreError::Unsupported(format!(
                    "VFP single-transfer double register d{} is out of bounds for 0x{word:08x}",
                    reg
                ))));
            }
            if l {
                let value = match self.read_u64_le(addr) {
                    Ok(value) => value,
                    Err(err) => return Some(Err(err)),
                };
                self.exec.vfp_d_regs[reg] = value;
            } else {
                let value = self.exec.vfp_d_regs[reg];
                if let Err(err) = self.write_u64_le(addr, value) {
                    return Some(Err(err));
                }
            }
            (reg as u32, reg as u32, format!("{{d{}}}", reg), "double")
        } else {
            let reg = ((vd << 1) | (d_bit as usize)) as usize;
            let single_capacity = self.exec.vfp_d_regs.len().saturating_mul(2);
            if reg >= single_capacity {
                return Some(Err(CoreError::Unsupported(format!(
                    "VFP single-transfer single register s{} is out of bounds for 0x{word:08x}",
                    reg
                ))));
            }
            if l {
                let value = match self.read_u32_le(addr) {
                    Ok(value) => value,
                    Err(err) => return Some(Err(err)),
                };
                self.vfp_set_s(reg, value);
            } else {
                let value = self.vfp_get_s(reg);
                if let Err(err) = self.write_u32_le(addr, value) {
                    return Some(Err(err));
                }
            }
            (reg as u32, reg as u32, format!("{{s{}}}", reg), "single")
        };

        if double_precision {
            self.exec.vfp_double_transfer_ops = self.exec.vfp_double_transfer_ops.saturating_add(1);
        } else {
            self.exec.vfp_single_transfer_ops = self.exec.vfp_single_transfer_ops.saturating_add(1);
        }
        if pc_base {
            self.exec.vfp_pc_base_ops = self.exec.vfp_pc_base_ops.saturating_add(1);
            if l {
                self.exec.vfp_pc_base_load_ops = self.exec.vfp_pc_base_load_ops.saturating_add(1);
            } else {
                self.exec.vfp_pc_base_store_ops = self.exec.vfp_pc_base_store_ops.saturating_add(1);
            }
            self.exec.vfp_last_pc_base_addr = Some(addr);
            self.exec.vfp_last_pc_base_word = Some(word);
        }
        self.exec.vfp_last_transfer_addr = Some(addr);
        self.exec.vfp_last_transfer_start_reg = Some(start_reg);
        self.exec.vfp_last_transfer_end_reg = Some(end_reg);
        self.exec.vfp_last_transfer_count = Some(1);
        self.exec.vfp_last_transfer_precision = Some(precision.to_string());
        self.exec.vfp_last_transfer_mode = Some(if l {
            if pc_base { "literal-load".to_string() } else { "base-load".to_string() }
        } else if pc_base {
            "literal-store".to_string()
        } else {
            "base-store".to_string()
        });
        self.exec.vfp_last_exact_decoder_branch = Some("generic-vfp-single-transfer".to_string());
        self.exec.vfp_last_exact_reason = Some("matched generic VFP single-transfer decoder".to_string());
        self.exec.vfp_last_exact_opcode = Some(format!(
            "word=0x{word:08x} base={} addr=0x{addr:08x} start={} end={} count=1 precision={} load={} pc_base={}",
            if pc_base {
                format!("pc@0x{base:08x}")
            } else {
                format!("r{}@0x{base:08x}", rn)
            },
            start_reg,
            end_reg,
            precision,
            l,
            pc_base,
        ));

        let op_name = if l { "vldr" } else { "vstr" };
        let base_label = if pc_base {
            format!("pc@0x{:08x}", base)
        } else {
            format!("r{}@0x{:08x}", rn, base)
        };
        let detail = format!(
            "{} {} [{}{}, #{}] addr=0x{:08x} precision={}",
            op_name,
            reg_text,
            base_label,
            if pc_base { "" } else { "" },
            if u { format!("+{}", offset) } else { format!("-{}", offset) },
            addr,
            precision,
        );
        self.exec.vfp_last_op = Some(detail.clone());
        self.diag.trace.push(format!("     ↳ {}", detail));
        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn try_exec_vfp_scalar_data_processing(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let masked = word & 0x0FB0_0F50;
        let double_precision = ((word >> 8) & 1) != 0;
        let op_name = match masked {
            0x0E00_0A00 | 0x0E00_0B00 => {
                if double_precision { "vmla.f64" } else { "vmla.f32" }
            }
            0x0E00_0A40 | 0x0E00_0B40 => {
                if double_precision { "vmls.f64" } else { "vmls.f32" }
            }
            0x0E10_0A00 | 0x0E10_0B00 => {
                if double_precision { "vnmls.f64" } else { "vnmls.f32" }
            }
            0x0E10_0A40 | 0x0E10_0B40 => {
                if double_precision { "vnmla.f64" } else { "vnmla.f32" }
            }
            0x0E20_0A00 | 0x0E20_0B00 => {
                if double_precision { "vmul.f64" } else { "vmul.f32" }
            }
            0x0E20_0A40 | 0x0E20_0B40 => {
                if double_precision { "vnmul.f64" } else { "vnmul.f32" }
            }
            0x0E30_0A00 | 0x0E30_0B00 => {
                if double_precision { "vadd.f64" } else { "vadd.f32" }
            }
            0x0E30_0A40 | 0x0E30_0B40 => {
                if double_precision { "vsub.f64" } else { "vsub.f32" }
            }
            0x0E80_0A00 | 0x0E80_0B00 => {
                if double_precision { "vdiv.f64" } else { "vdiv.f32" }
            }
            _ => return None,
        };

        let d_bit = ((word >> 22) & 1) != 0;
        let vd = ((word >> 12) & 0xF) as usize;
        let n_bit = ((word >> 7) & 1) != 0;
        let vn = ((word >> 16) & 0xF) as usize;
        let m_bit = ((word >> 5) & 1) != 0;
        let vm = (word & 0xF) as usize;

        if double_precision {
            let dd = ((d_bit as usize) << 4) | vd;
            let dn = ((n_bit as usize) << 4) | vn;
            let dm = ((m_bit as usize) << 4) | vm;
            let lhs = self.vfp_get_d_f64(dd);
            let rhs_n = self.vfp_get_d_f64(dn);
            let rhs_m = self.vfp_get_d_f64(dm);
            let result = match masked {
                0x0E00_0B00 => lhs + (rhs_n * rhs_m),
                0x0E00_0B40 => lhs - (rhs_n * rhs_m),
                0x0E10_0B00 => (rhs_n * rhs_m) - lhs,
                0x0E10_0B40 => -(lhs + (rhs_n * rhs_m)),
                0x0E20_0B00 => rhs_n * rhs_m,
                0x0E20_0B40 => -(rhs_n * rhs_m),
                0x0E30_0B00 => rhs_n + rhs_m,
                0x0E30_0B40 => rhs_n - rhs_m,
                0x0E80_0B00 => rhs_n / rhs_m,
                _ => unreachable!(),
            };
            self.vfp_set_d_f64(dd, result);
            let detail = format!("{} d{}, d{}, d{}", op_name, dd, dn, dm);
            self.exec.vfp_last_op = Some(detail.clone());
            self.diag.trace.push(format!("     ↳ {} = {:e}", detail, result));
        } else {
            let sd = (vd << 1) | (d_bit as usize);
            let sn = (vn << 1) | (n_bit as usize);
            let sm = (vm << 1) | (m_bit as usize);
            let lhs = self.vfp_get_s_f32(sd);
            let rhs_n = self.vfp_get_s_f32(sn);
            let rhs_m = self.vfp_get_s_f32(sm);
            let result = match masked {
                0x0E00_0A00 => lhs + (rhs_n * rhs_m),
                0x0E00_0A40 => lhs - (rhs_n * rhs_m),
                0x0E10_0A00 => (rhs_n * rhs_m) - lhs,
                0x0E10_0A40 => -(lhs + (rhs_n * rhs_m)),
                0x0E20_0A00 => rhs_n * rhs_m,
                0x0E20_0A40 => -(rhs_n * rhs_m),
                0x0E30_0A00 => rhs_n + rhs_m,
                0x0E30_0A40 => rhs_n - rhs_m,
                0x0E80_0A00 => rhs_n / rhs_m,
                _ => unreachable!(),
            };
            self.vfp_set_s_f32(sd, result);
            let detail = format!("{} s{}, s{}, s{}", op_name, sd, sn, sm);
            self.exec.vfp_last_op = Some(detail.clone());
            self.diag.trace.push(format!("     ↳ {} = {:e}", detail, result));
        }

        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn try_exec_vfp_vmov_arm_sreg(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        if (word & 0x0DA0_0E70) != 0x0C00_0A10 {
            return None;
        }
        let l = ((word >> 20) & 1) != 0;
        let n_bit = ((word >> 7) & 1) != 0;
        let vn = ((word >> 16) & 0xF) as usize;
        let rt = ((word >> 12) & 0xF) as usize;
        let sreg = (vn << 1) | (n_bit as usize);
        if rt == 15 {
            return Some(Err(CoreError::Unsupported(format!(
                "vmov between ARM register PC and s{} is not implemented yet for 0x{word:08x}",
                sreg
            ))));
        }
        let detail = if l {
            let value = self.vfp_get_s(sreg);
            self.cpu.regs[rt] = value;
            format!("vmov r{}, s{}", rt, sreg)
        } else {
            let value = self.cpu.regs[rt];
            self.vfp_set_s(sreg, value);
            format!("vmov s{}, r{}", sreg, rt)
        };
        self.exec.vfp_last_op = Some(detail.clone());
        self.diag.trace.push(format!("     ↳ {}", detail));
        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn try_exec_vfp_vmov_scalar(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let masked = word & 0x0FBF_0FD0;
        if masked != 0x0EB0_0A40 && masked != 0x0EB0_0B40 {
            return None;
        }
        let d_bit = ((word >> 22) & 1) != 0;
        let vd = ((word >> 12) & 0xF) as usize;
        let m_bit = ((word >> 5) & 1) != 0;
        let vm = (word & 0xF) as usize;
        let double_precision = ((word >> 8) & 1) != 0;
        let detail = if double_precision {
            let dd = ((d_bit as usize) << 4) | vd;
            let dm = ((m_bit as usize) << 4) | vm;
            let value = self.vfp_get_d_f64(dm);
            self.vfp_set_d_f64(dd, value);
            format!("vmov.f64 d{}, d{}", dd, dm)
        } else {
            let sd = (vd << 1) | (d_bit as usize);
            let sm = (vm << 1) | (m_bit as usize);
            let value = self.vfp_get_s(sm);
            self.vfp_set_s(sd, value);
            format!("vmov.f32 s{}, s{}", sd, sm)
        };
        self.exec.vfp_last_op = Some(detail.clone());
        self.diag.trace.push(format!("     ↳ {}", detail));
        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn try_exec_vfp_unary_scalar_data_processing(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let masked = word & 0x0FBF_0FD0;
        let op_name = match masked {
            0x0EB0_0AC0 => "vabs.f32",
            0x0EB1_0A40 => "vneg.f32",
            0x0EB1_0AC0 => "vsqrt.f32",
            0x0EB0_0BC0 => "vabs.f64",
            0x0EB1_0B40 => "vneg.f64",
            0x0EB1_0BC0 => "vsqrt.f64",
            _ => return None,
        };

        let double_precision = ((word >> 8) & 1) != 0;
        let d_bit = ((word >> 22) & 1) != 0;
        let vd = ((word >> 12) & 0xF) as usize;
        let m_bit = ((word >> 5) & 1) != 0;
        let vm = (word & 0xF) as usize;

        if double_precision {
            let dd = ((d_bit as usize) << 4) | vd;
            let dm = ((m_bit as usize) << 4) | vm;
            let src = self.vfp_get_d_f64(dm);
            let result = match masked {
                0x0EB0_0BC0 => src.abs(),
                0x0EB1_0B40 => -src,
                0x0EB1_0BC0 => src.sqrt(),
                _ => unreachable!(),
            };
            self.vfp_set_d_f64(dd, result);
            let detail = format!("{} d{}, d{}", op_name, dd, dm);
            self.exec.vfp_last_op = Some(detail.clone());
            self.diag.trace.push(format!("     ↳ {} = {:e}", detail, result));
        } else {
            let sd = (vd << 1) | (d_bit as usize);
            let sm = (vm << 1) | (m_bit as usize);
            let src = self.vfp_get_s_f32(sm);
            let result = match masked {
                0x0EB0_0AC0 => src.abs(),
                0x0EB1_0A40 => -src,
                0x0EB1_0AC0 => src.sqrt(),
                _ => unreachable!(),
            };
            self.vfp_set_s_f32(sd, result);
            let detail = format!("{} s{}, s{}", op_name, sd, sm);
            self.exec.vfp_last_op = Some(detail.clone());
            self.diag.trace.push(format!("     ↳ {} = {:e}", detail, result));
        }

        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn try_exec_vfp_convert_between_float_int(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let masked = word & 0x0FBF_0FD0;
        let op_name = match masked {
            0x0EB8_0A40 => "vcvt.f32.u32",
            0x0EB8_0AC0 => "vcvt.f32.s32",
            0x0EBC_0AC0 => "vcvt.u32.f32",
            0x0EBD_0AC0 => "vcvt.s32.f32",
            // ARM/VFP encodes the float<->float width conversions opposite to what a
            // naive masked-pattern reading suggests here. 0x0EB7_0AC0 is f64<-f32,
            // while 0x0EB7_0BC0 is f32<-f64.
            0x0EB7_0AC0 => "vcvt.f64.f32",
            0x0EB8_0B40 => "vcvt.f64.u32",
            0x0EB8_0BC0 => "vcvt.f64.s32",
            0x0EBC_0BC0 => "vcvt.u32.f64",
            0x0EBD_0BC0 => "vcvt.s32.f64",
            0x0EB7_0BC0 => "vcvt.f32.f64",
            _ => return None,
        };
        let double_precision = ((word >> 8) & 1) != 0;
        let d_bit = ((word >> 22) & 1) != 0;
        let vd = ((word >> 12) & 0xF) as usize;
        let m_bit = ((word >> 5) & 1) != 0;
        let vm = (word & 0xF) as usize;
        let sd = (vd << 1) | (d_bit as usize);
        let sm = (vm << 1) | (m_bit as usize);
        let dd = ((d_bit as usize) << 4) | vd;
        let dm = ((m_bit as usize) << 4) | vm;

        let detail = match masked {
            0x0EB8_0A40 => {
                let value = self.vfp_get_s(sm) as f32;
                self.vfp_set_s_f32(sd, value);
                format!("{} s{}, s{} = {:e}", op_name, sd, sm, value)
            }
            0x0EB8_0AC0 => {
                let value = (self.vfp_get_s(sm) as i32) as f32;
                self.vfp_set_s_f32(sd, value);
                format!("{} s{}, s{} = {:e}", op_name, sd, sm, value)
            }
            0x0EBC_0AC0 => {
                let src = self.vfp_get_s_f32(sm);
                let value = if src.is_nan() || src <= 0.0 {
                    0u32
                } else if src >= u32::MAX as f32 {
                    u32::MAX
                } else {
                    src.round() as u32
                };
                self.vfp_set_s(sd, value);
                format!("{} s{}, s{} = 0x{:08x}", op_name, sd, sm, value)
            }
            0x0EBD_0AC0 => {
                let src = self.vfp_get_s_f32(sm);
                let value = if src.is_nan() {
                    0i32
                } else if src <= i32::MIN as f32 {
                    i32::MIN
                } else if src >= i32::MAX as f32 {
                    i32::MAX
                } else {
                    src.round() as i32
                };
                self.vfp_set_s(sd, value as u32);
                format!("{} s{}, s{} = 0x{:08x}", op_name, sd, sm, value as u32)
            }
            0x0EB7_0AC0 => {
                let src = self.vfp_get_s_f32(sm);
                let value = src as f64;
                self.vfp_set_d_f64(dd, value);
                format!("{} d{}, s{} = {:e}", op_name, dd, sm, value)
            }
            0x0EB8_0B40 => {
                let value = self.vfp_get_s(sm) as f64;
                self.vfp_set_d_f64(dd, value);
                format!("{} d{}, s{} = {:e}", op_name, dd, sm, value)
            }
            0x0EB8_0BC0 => {
                let value = (self.vfp_get_s(sm) as i32) as f64;
                self.vfp_set_d_f64(dd, value);
                format!("{} d{}, s{} = {:e}", op_name, dd, sm, value)
            }
            0x0EBC_0BC0 => {
                let src = self.vfp_get_d_f64(dm);
                let value = if src.is_nan() || src <= 0.0 {
                    0u32
                } else if src >= u32::MAX as f64 {
                    u32::MAX
                } else {
                    src.round() as u32
                };
                self.vfp_set_s(sd, value);
                format!("{} s{}, d{} = 0x{:08x}", op_name, sd, dm, value)
            }
            0x0EBD_0BC0 => {
                let src = self.vfp_get_d_f64(dm);
                let value = if src.is_nan() {
                    0i32
                } else if src <= i32::MIN as f64 {
                    i32::MIN
                } else if src >= i32::MAX as f64 {
                    i32::MAX
                } else {
                    src.round() as i32
                };
                self.vfp_set_s(sd, value as u32);
                format!("{} s{}, d{} = 0x{:08x}", op_name, sd, dm, value as u32)
            }
            0x0EB7_0BC0 => {
                let src = self.vfp_get_d_f64(dm);
                let value = src as f32;
                self.vfp_set_s_f32(sd, value);
                format!("{} s{}, d{} = {:e}", op_name, sd, dm, value)
            }
            _ => unreachable!(),
        };

        self.diag.trace.push(format!("     ↳ {}", detail));
        self.exec.vfp_last_op = Some(format!(
            "{} {}",
            detail,
            if double_precision { "[double]" } else { "[single]" }
        ));
        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn try_exec_vfp_compare(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        let masked = word & 0x0FBE_0FD0;
        let double_precision = ((word >> 8) & 1) != 0;
        match masked {
            0x0EB4_0A40 | 0x0EB4_0AC0 => {
                let d_bit = ((word >> 22) & 1) != 0;
                let vd = ((word >> 12) & 0xF) as usize;
                let m_bit = ((word >> 5) & 1) != 0;
                let vm = (word & 0xF) as usize;
                let sd = (vd << 1) | (d_bit as usize);
                let lhs = self.vfp_get_s_f32(sd);
                let zero_rhs = (word & 0x6F) == 0x40;
                let (rhs, rhs_text) = if zero_rhs {
                    (0.0f32, "#0".to_string())
                } else {
                    let sm = (vm << 1) | (m_bit as usize);
                    (self.vfp_get_s_f32(sm), format!("s{}", sm))
                };
                self.vfp_set_cmp_flags_f32(lhs, rhs);
                let detail = if masked == 0x0EB4_0AC0 {
                    format!("vcmpe.f32 s{}, {}", sd, rhs_text)
                } else {
                    format!("vcmp.f32 s{}, {}", sd, rhs_text)
                };
                self.exec.vfp_last_op = Some(detail.clone());
                self.diag.trace.push(format!(
                    "     ↳ {} => N={} Z={} C={} V={}",
                    detail,
                    self.cpu.flags.n,
                    self.cpu.flags.z,
                    self.cpu.flags.c,
                    self.cpu.flags.v
                ));
                self.cpu.regs[15] = current_pc.wrapping_add(4);
                Some(Ok(StepControl::Continue))
            }
            0x0EB4_0B40 | 0x0EB4_0BC0 if double_precision => {
                let d_bit = ((word >> 22) & 1) != 0;
                let vd = ((word >> 12) & 0xF) as usize;
                let m_bit = ((word >> 5) & 1) != 0;
                let vm = (word & 0xF) as usize;
                let dd = ((d_bit as usize) << 4) | vd;
                let lhs = self.vfp_get_d_f64(dd);
                let zero_rhs = (word & 0x6F) == 0x40;
                let (rhs, rhs_text) = if zero_rhs {
                    (0.0f64, "#0".to_string())
                } else {
                    let dm = ((m_bit as usize) << 4) | vm;
                    (self.vfp_get_d_f64(dm), format!("d{}", dm))
                };
                self.vfp_set_cmp_flags_f64(lhs, rhs);
                let detail = if masked == 0x0EB4_0BC0 {
                    format!("vcmpe.f64 d{}, {}", dd, rhs_text)
                } else {
                    format!("vcmp.f64 d{}, {}", dd, rhs_text)
                };
                self.exec.vfp_last_op = Some(detail.clone());
                self.diag.trace.push(format!(
                    "     ↳ {} => N={} Z={} C={} V={}",
                    detail,
                    self.cpu.flags.n,
                    self.cpu.flags.z,
                    self.cpu.flags.c,
                    self.cpu.flags.v
                ));
                self.cpu.regs[15] = current_pc.wrapping_add(4);
                Some(Ok(StepControl::Continue))
            }
            _ => None,
        }
    }

    fn try_exec_vfp_vmrs_apsr(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        if word != 0xEEF1_FA10 {
            return None;
        }
        let detail = "vmrs APSR_nzcv, fpscr".to_string();
        self.exec.vfp_last_op = Some(detail.clone());
        self.diag.trace.push(format!(
            "     ↳ {} => using emulated compare flags N={} Z={} C={} V={}",
            detail,
            self.cpu.flags.n,
            self.cpu.flags.z,
            self.cpu.flags.c,
            self.cpu.flags.v
        ));
        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }

    fn is_vfp_load_store_multiple(word: u32) -> bool {
        (word & 0x0E00_0E00) == 0x0C00_0A00
    }

    fn try_exec_vfp_load_store_multiple(&mut self, word: u32, current_pc: u32) -> Option<CoreResult<StepControl>> {
        if !Self::is_vfp_load_store_multiple(word) {
            return None;
        }

        self.exec.vfp_last_exact_decoder_branch = Some("generic-vfp-load-store-multiple".to_string());
        self.exec.vfp_last_exact_reason = Some("entered generic VFP load/store multiple decoder".to_string());

        let p = ((word >> 24) & 1) != 0;
        let u = ((word >> 23) & 1) != 0;
        let d_bit = ((word >> 22) & 1) != 0;
        let w = ((word >> 21) & 1) != 0;
        let l = ((word >> 20) & 1) != 0;
        let rn = ((word >> 16) & 0xF) as usize;
        let vd = ((word >> 12) & 0xF) as usize;
        let double_precision = ((word >> 8) & 1) != 0;
        let imm8 = (word & 0xFF) as u32;

        let pc_base = rn == 15;
        if pc_base && w {
            return Some(Err(CoreError::Unsupported(format!(
                "VFP multi with PC base writeback is not implemented yet for 0x{word:08x}"
            ))));
        }
        if pc_base && !l {
            return Some(Err(CoreError::Unsupported(format!(
                "VFP multi store with PC base is not implemented yet for 0x{word:08x}"
            ))));
        }
        if imm8 == 0 {
            return Some(Err(CoreError::Unsupported(format!(
                "empty VFP register list is not implemented yet for 0x{word:08x}"
            ))));
        }

        let base = self.reg_operand(rn, current_pc);
        let total_words = imm8;
        let start_addr = match (p, u) {
            (false, true) => base,
            (true, true) => base.wrapping_add(4),
            (false, false) => base.wrapping_sub(4 * total_words.saturating_sub(1)),
            (true, false) => base.wrapping_sub(4 * total_words),
        };
        let final_base = if u {
            base.wrapping_add(4 * total_words)
        } else {
            base.wrapping_sub(4 * total_words)
        };

        let mut addr = start_addr;
        let reg_list_text;
        let element_bytes = if double_precision { 8u32 } else { 4u32 };

        if double_precision {
            if (imm8 & 1) != 0 {
                return Some(Err(CoreError::Unsupported(format!(
                    "odd-word VFP double list is not implemented yet for 0x{word:08x}"
                ))));
            }
            let start_reg = (((d_bit as usize) << 4) | vd) as usize;
            let reg_count = (imm8 / 2) as usize;
            if start_reg + reg_count > self.exec.vfp_d_regs.len() {
                return Some(Err(CoreError::Unsupported(format!(
                    "VFP double register range d{}..d{} is out of bounds for 0x{word:08x}",
                    start_reg,
                    start_reg + reg_count.saturating_sub(1)
                ))));
            }
            for offset in 0..reg_count {
                let reg = start_reg + offset;
                if l {
                    let value = match self.read_u64_le(addr) {
                        Ok(value) => value,
                        Err(err) => return Some(Err(err)),
                    };
                    self.exec.vfp_d_regs[reg] = value;
                } else {
                    let value = self.exec.vfp_d_regs[reg];
                    if let Err(err) = self.write_u64_le(addr, value) {
                        return Some(Err(err));
                    }
                }
                addr = addr.wrapping_add(8);
            }
            reg_list_text = format_vfp_reg_list(start_reg as u32, reg_count as u32, true);
        } else {
            let start_reg = ((vd << 1) | (d_bit as usize)) as usize;
            let reg_count = imm8 as usize;
            let single_capacity = self.exec.vfp_d_regs.len().saturating_mul(2);
            let end_reg_inclusive = start_reg + reg_count.saturating_sub(1);
            if start_reg + reg_count > single_capacity {
                self.exec.vfp_last_single_range = Some(format!(
                    "s{}..s{} count={} capacity={} word=0x{word:08x}",
                    start_reg,
                    end_reg_inclusive,
                    reg_count,
                    single_capacity
                ));
                self.exec.vfp_last_exact_reason = Some(format!(
                    "generic multi decoder produced single-register range s{}..s{} count={} capacity={} for 0x{word:08x}",
                    start_reg,
                    end_reg_inclusive,
                    reg_count,
                    single_capacity,
                ));
                return Some(Err(CoreError::Unsupported(format!(
                    "VFP single register range s{}..s{} exceeds capacity s0..s{} for 0x{word:08x}",
                    start_reg,
                    end_reg_inclusive,
                    single_capacity.saturating_sub(1)
                ))));
            }
            for offset in 0..reg_count {
                let reg = start_reg + offset;
                if l {
                    let value = match self.read_u32_le(addr) {
                        Ok(value) => value,
                        Err(err) => return Some(Err(err)),
                    };
                    self.vfp_set_s(reg, value);
                } else {
                    let value = self.vfp_get_s(reg);
                    if let Err(err) = self.write_u32_le(addr, value) {
                        return Some(Err(err));
                    }
                }
                addr = addr.wrapping_add(4);
            }
            self.exec.vfp_single_range_ops = self.exec.vfp_single_range_ops.saturating_add(1);
            self.exec.vfp_last_single_range = Some(format!(
                "s{}..s{} count={} capacity={}",
                start_reg,
                end_reg_inclusive,
                reg_count,
                single_capacity
            ));
            reg_list_text = format_vfp_reg_list(start_reg as u32, reg_count as u32, false);
        }

        if w {
            self.cpu.regs[rn] = final_base;
        }

        self.exec.vfp_multi_ops = self.exec.vfp_multi_ops.saturating_add(1);
        if l {
            self.exec.vfp_load_multi_ops = self.exec.vfp_load_multi_ops.saturating_add(1);
        } else {
            self.exec.vfp_store_multi_ops = self.exec.vfp_store_multi_ops.saturating_add(1);
        }
        let last_addr = addr.wrapping_sub(element_bytes);
        self.exec.vfp_last_start_addr = Some(start_addr);
        self.exec.vfp_last_end_addr = Some(last_addr);
        if pc_base {
            self.exec.vfp_pc_base_ops = self.exec.vfp_pc_base_ops.saturating_add(1);
            if l {
                self.exec.vfp_pc_base_load_ops = self.exec.vfp_pc_base_load_ops.saturating_add(1);
            } else {
                self.exec.vfp_pc_base_store_ops = self.exec.vfp_pc_base_store_ops.saturating_add(1);
            }
            self.exec.vfp_last_pc_base_addr = Some(start_addr);
            self.exec.vfp_last_pc_base_word = Some(word);
        }

        let op_name = if pc_base && l {
            "vldm-pc"
        } else if pc_base {
            "vstm-pc"
        } else if !l && rn == 13 && p && !u && w {
            "vpush"
        } else if l && rn == 13 && !p && u && w {
            "vpop"
        } else if l {
            "vldm"
        } else {
            "vstm"
        };
        let base_label = if pc_base {
            format!("pc@0x{:08x}", base)
        } else {
            format!("r{}", rn)
        };
        let detail = format!(
            "{} {} base={} start=0x{:08x} end=0x{:08x} writeback={}",
            op_name,
            reg_list_text,
            base_label,
            start_addr,
            last_addr,
            if w { "yes" } else { "no" },
        );
        self.exec.vfp_last_op = Some(detail.clone());
        self.diag.trace.push(format!("     ↳ {}", detail));

        self.cpu.regs[15] = current_pc.wrapping_add(4);
        Some(Ok(StepControl::Continue))
    }
}
