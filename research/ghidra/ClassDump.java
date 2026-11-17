// Decompiles the code of SimGroup classes: constructor, attribute reader,
// message handler and the other methods a class overrides.
// args: outDir Name:factoryAddr [Name:factoryAddr ...]
//
// The factory of a class (third argument of the registration 0x4833A0) is
// `alloc(size); return ctor(arg)`. The constructor calls the constructor of
// the base class and then stores its own method tables into the object
// (`mov dword ptr [reg+off], table`). For every class the script writes
// <outDir>/<Name>/index.txt: the chain of constructors, the method tables of
// each with their slots, and which slots the class overrides; and one .c file
// per overridden method plus the functions they call that lie near them.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;

public class ClassDump extends GhidraScript {
    Memory mem;
    DecompInterface di;

    Function ensure(long addr) throws Exception {
        Address a = toAddr(addr);
        Function f = getFunctionAt(a);
        if (f != null) return f;
        if (getFunctionContaining(a) != null) return null;
        disassemble(a);
        return createFunction(a, null);
    }

    boolean isCode(long v) {
        MemoryBlock b = mem.getBlock(toAddr(v));
        return b != null && b.isExecute();
    }

    boolean isTable(long v) {
        MemoryBlock b = mem.getBlock(toAddr(v));
        if (b == null || b.isExecute()) return false;
        try {
            return isCode(mem.getInt(toAddr(v)) & 0xffffffffL);
        } catch (Exception e) {
            return false;
        }
    }

    List<Long> slots(long table) {
        List<Long> out = new ArrayList<>();
        for (int k = 0; k < 64; k++) {
            long v;
            try {
                v = mem.getInt(toAddr(table + 4L * k)) & 0xffffffffL;
            } catch (Exception e) {
                break;
            }
            if (!isCode(v)) break;
            // a table ends where another one is referenced from code
            if (k > 0 && getReferencesTo(toAddr(table + 4L * k)).length > 0) break;
            out.add(v);
        }
        return out;
    }

    // calls of a function, in address order
    List<Long> calls(Function f) {
        List<Long> out = new ArrayList<>();
        InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            if (!ins.getFlowType().isCall()) continue;
            for (Address t : ins.getFlows()) out.add(t.getOffset());
        }
        return out;
    }

    // offset -> table stored by `mov [reg+off], imm` in the function
    Map<Long, Long> tables(Function f) {
        Map<Long, Long> out = new TreeMap<>();
        InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            if (!ins.getMnemonicString().equals("MOV") || ins.getNumOperands() != 2) continue;
            Object[] src = ins.getOpObjects(1);
            if (src.length != 1 || !(src[0] instanceof Scalar)) continue;
            long v = ((Scalar) src[0]).getUnsignedValue();
            if (!isTable(v)) continue;
            long off = 0;
            for (Object o : ins.getOpObjects(0)) if (o instanceof Scalar) off = ((Scalar) o).getUnsignedValue();
            out.put(off, v);
        }
        return out;
    }

    void decomp(Function f, File dir, String tag) throws Exception {
        File out = new File(dir, f.getEntryPoint().toString() + ".c");
        if (out.exists()) return;
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n";
        try (PrintWriter pw = new PrintWriter(out)) {
            pw.print("// " + tag + " @ " + f.getEntryPoint() + "\n" + c);
        }
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        mem = currentProgram.getMemory();
        di = new DecompInterface();
        di.openProgram(currentProgram);
        for (int i = 1; i < a.length; i++) {
            String[] p = a[i].split(":");
            File dir = new File(a[0], p[0]);
            dir.mkdirs();
            try (PrintWriter idx = new PrintWriter(new File(dir, "index.txt"))) {
                Function fac = ensure(Long.parseLong(p[1].replace("0x", ""), 16));
                if (fac == null) { idx.println("no factory function"); continue; }
                List<Long> fc = calls(fac);
                if (fc.isEmpty()) { idx.println("factory has no calls"); continue; }
                idx.println("class " + p[0] + " factory " + fac.getEntryPoint());
                // chain of constructors: derived first
                List<Function> chain = new ArrayList<>();
                List<Map<Long, Long>> tabs = new ArrayList<>();
                long c = fc.get(fc.size() - 1);
                for (int depth = 0; depth < 8; depth++) {
                    Function cf = ensure(c);
                    if (cf == null) break;
                    Map<Long, Long> t = tables(cf);
                    if (t.isEmpty() && depth > 0) break;
                    chain.add(cf);
                    tabs.add(t);
                    List<Long> cc = calls(cf);
                    if (cc.isEmpty()) break;
                    c = cc.get(0);
                }
                Set<Long> todo = new LinkedHashSet<>();
                for (int k = 0; k < chain.size(); k++) {
                    idx.println("ctor[" + k + "] " + chain.get(k).getEntryPoint());
                    for (Map.Entry<Long, Long> e : tabs.get(k).entrySet()) {
                        List<Long> s = slots(e.getValue());
                        Long baseTab = k + 1 < tabs.size() ? tabs.get(k + 1).get(e.getKey()) : null;
                        List<Long> bs = baseTab != null ? slots(baseTab) : new ArrayList<>();
                        StringBuilder sb = new StringBuilder();
                        for (int j = 0; j < s.size(); j++) {
                            boolean own = j >= bs.size() || !bs.get(j).equals(s.get(j));
                            sb.append(String.format(" %d:%08x%s", j, s.get(j), own ? "*" : ""));
                            // the class itself and its direct base, unless the base is the root of the chain
                            if (own && (k == 0 || (k == 1 && chain.size() > 2))) todo.add(s.get(j));
                        }
                        idx.println(String.format("  table +0x%x %08x:%s", e.getKey(), e.getValue(), sb));
                    }
                }
                if (chain.isEmpty()) continue;
                decomp(chain.get(0), dir, "ctor");
                long lo = Long.MAX_VALUE, hi = 0;
                List<Function> own = new ArrayList<>();
                for (long m : todo) {
                    Function f = ensure(m);
                    if (f == null) { idx.println("no function at " + Long.toHexString(m)); continue; }
                    own.add(f);
                    if (f.getBody().getNumAddresses() > 8) { lo = Math.min(lo, m); hi = Math.max(hi, m); }
                    decomp(f, dir, "method");
                }
                // callees that lie near the methods of the class (helpers of the class)
                Set<Long> near = new TreeSet<>();
                for (Function f : own)
                    for (long t : calls(f))
                        if (t >= lo - 0x1800 && t <= hi + 0x1800 && !todo.contains(t)) near.add(t);
                for (long t : near) {
                    Function f = ensure(t);
                    if (f == null) continue;
                    decomp(f, dir, "callee");
                    idx.println("callee " + f.getEntryPoint());
                }
            }
        }
        println("done");
    }
}
