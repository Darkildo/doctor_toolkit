// Decompiles every function that calls one of the given functions.
// args: outDir addr1 [addr2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import java.io.*;
import java.util.*;

public class Callers extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        File outDir = new File(a[0]);
        outDir.mkdirs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        Set<Function> callers = new LinkedHashSet<>();
        for (int i = 1; i < a.length; i++) {
            Function f = getFunctionAt(toAddr(Long.parseLong(a[i].replace("0x", ""), 16)));
            if (f == null) { println("no function at " + a[i]); continue; }
            callers.addAll(f.getCallingFunctions(monitor));
        }
        for (Function f : callers) {
            DecompileResults r = di.decompileFunction(f, 60, monitor);
            String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n";
            try (PrintWriter pw = new PrintWriter(new File(outDir, f.getEntryPoint().toString() + ".c"))) {
                pw.print("// " + f.getName() + " @ " + f.getEntryPoint() + "\n" + c);
            }
        }
        println("done " + callers.size());
    }
}
