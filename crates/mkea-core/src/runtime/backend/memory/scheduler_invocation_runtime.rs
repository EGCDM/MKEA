impl MemoryArm32Backend {
    // Synthetic NSMethodSignature / NSInvocation behavior lives here instead of
    // staying buried inside cocos_runtime.rs.

    fn create_synthetic_method_signature(&mut self, receiver: u32, selector_name: Option<String>, objc_types: Option<String>, origin: &str) -> u32 {
        let obj = self.objc_hle_alloc_like(receiver, 0, origin);
        let label = if let Some(selector_name) = selector_name.as_deref() {
            format!("NSMethodSignature.synthetic<{}>", selector_name)
        } else if let Some(objc_types) = objc_types.as_deref() {
            format!("NSMethodSignature.synthetic<{}>", objc_types)
        } else {
            "NSMethodSignature.synthetic".to_string()
        };
        self.diag.object_labels.insert(obj, label);
        self.runtime.scheduler.invocations.method_signatures.insert(obj, SyntheticMethodSignature { selector_name, objc_types });
        obj
    }

    fn create_synthetic_invocation(&mut self, receiver: u32, signature: u32, origin: &str) -> u32 {
        let obj = self.objc_hle_alloc_like(receiver, 0, origin);
        self.diag.object_labels.insert(obj, format!("NSInvocation.synthetic#{}", self.runtime.scheduler.invocations.invocations.len()));
        self.runtime.scheduler.invocations.invocations.insert(obj, SyntheticInvocation {
            signature,
            ..SyntheticInvocation::default()
        });
        obj
    }

    fn synthetic_invocation_argument_value(&self, location: u32) -> u32 {
        if location == 0 {
            0
        } else {
            self.read_u32_le(location).unwrap_or(location)
        }
    }

    fn trace_synthetic_invocation(&mut self, event: impl Into<String>) {
        self.push_callback_trace(format!("nsinvocation {}", event.into()));
    }

}
