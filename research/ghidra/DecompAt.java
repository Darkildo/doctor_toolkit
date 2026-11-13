// Decompiles the function containing each address, creating it when Ghidra has none:
// walks back to 16-byte aligned starts after padding and tries them in turn.
// args: outDir addr1 [addr2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import java.io.*;

public class DecompAt extends GhidraScript {
    Function ensure(Address x) throws Exception {
        Function f = getFunctionContaining(x);
        if (f != null) return f;
        long off = x.getOffset() & ~0xfL;
        for (int tries = 0; tries < 400; tries++, off -= 16) {
            Address a = toAddr(off);
            byte prev = getByte(a.subtract(1));
            if (prev != (byte) 0xcc && prev != (byte) 0x90 && prev != (byte) 0xc3) continue;
            if (getFunctionAt(a) != null) break;
            Function other = getFunctionContaining(a);
            if (other != null) break;
            disassemble(a);
            Function g = createFunction(a, null);
            if (g == null) continue;
            if (g.getBody().contains(x)) return g;
            removeFunction(g);
        }
        return null;
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        File outDir = new File(a[0]);
        outDir.mkdirs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (int i = 1; i < a.length; i++) {
            Address x = toAddr(Long.parseLong(a[i].replace("0x", ""), 16));
            Function f = ensure(x);
            if (f == null) { println("no function for " + x); continue; }
            DecompileResults r = di.decompileFunction(f, 120, monitor);
            String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n";
            try (PrintWriter pw = new PrintWriter(new File(outDir, f.getEntryPoint().toString() + ".c"))) {
                pw.print("// " + f.getName() + " @ " + f.getEntryPoint() + " for " + x + "\n" + c);
            }
        }
        println("done");
    }
}
