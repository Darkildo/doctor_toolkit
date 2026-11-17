// Decompiles every function Ghidra has in [start, end) into one file per range.
// args: outDir start1 end1 [start2 end2 ...]   -> outDir/range_<start>.c
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import java.io.*;

public class DecompRange extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        File outDir = new File(a[0]);
        outDir.mkdirs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (int i = 1; i + 1 < a.length; i += 2) {
            Address s = toAddr(Long.parseLong(a[i].replace("0x", ""), 16));
            Address e = toAddr(Long.parseLong(a[i + 1].replace("0x", ""), 16));
            try (PrintWriter pw = new PrintWriter(new File(outDir, "range_" + s + ".c"))) {
                FunctionIterator it = currentProgram.getFunctionManager().getFunctions(s, true);
                while (it.hasNext()) {
                    Function f = it.next();
                    if (f.getEntryPoint().compareTo(e) >= 0) break;
                    DecompileResults r = di.decompileFunction(f, 120, monitor);
                    String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n";
                    pw.print("// ==== " + f.getName() + " @ " + f.getEntryPoint() + "\n" + c);
                }
            }
        }
        println("done");
    }
}
