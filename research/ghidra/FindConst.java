// Finds instructions with given 32-bit scalar operands; decompiles the containing functions.
// args: outDir const1 [const2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import java.io.*;
import java.util.*;

public class FindConst extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        File outDir = new File(a[0]);
        outDir.mkdirs();
        Map<Long, String> want = new HashMap<>();
        for (int i = 1; i < a.length; i++) want.put(Long.parseLong(a[i].replace("0x", ""), 16), a[i]);
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        Set<Function> done = new HashSet<>();
        PrintWriter idx = new PrintWriter(new File(outDir, "index.txt"));
        InstructionIterator it = currentProgram.getListing().getInstructions(true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            for (int op = 0; op < ins.getNumOperands(); op++) {
                for (Object o : ins.getOpObjects(op)) {
                    if (!(o instanceof Scalar)) continue;
                    long v = ((Scalar) o).getUnsignedValue() & 0xffffffffL;
                    if (!want.containsKey(v)) continue;
                    Function f = getFunctionContaining(ins.getAddress());
                    idx.println(want.get(v) + " " + ins.getAddress() + " " + (f == null ? "none" : f.getEntryPoint().toString()));
                    if (f != null && done.add(f)) {
                        DecompileResults r = di.decompileFunction(f, 120, monitor);
                        String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n";
                        try (PrintWriter pw = new PrintWriter(new File(outDir, f.getEntryPoint().toString() + ".c"))) {
                            pw.print("// " + f.getName() + " @ " + f.getEntryPoint() + "\n" + c);
                        }
                    }
                }
            }
        }
        idx.close();
        println("done " + done.size());
    }
}
