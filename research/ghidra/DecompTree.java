// Decompile functions at given addresses plus callees up to a depth.
// args: outDir depth addr1 [addr2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import java.io.*;
import java.util.*;

public class DecompTree extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        int depth = Integer.parseInt(args[1]);
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        FunctionManager fm = currentProgram.getFunctionManager();
        Set<Address> seen = new HashSet<>();
        Deque<Object[]> q = new ArrayDeque<>();
        for (int i = 2; i < args.length; i++) {
            Address a = toAddr(Long.parseLong(args[i].replace("0x", ""), 16));
            Function f = fm.getFunctionContaining(a);
            if (f == null) {
                disassemble(a);
                f = createFunction(a, null);
            }
            if (f == null) { println("no function at " + a); continue; }
            q.add(new Object[]{f, 0});
        }
        while (!q.isEmpty()) {
            Object[] e = q.poll();
            Function f = (Function) e[0];
            int d = (Integer) e[1];
            if (f.isThunk()) f = f.getThunkedFunction(true);
            if (!seen.add(f.getEntryPoint())) continue;
            DecompileResults r = di.decompileFunction(f, 120, monitor);
            String c = r.decompileCompleted() ? r.getDecompiledFunction().getC() : ("// failed: " + r.getErrorMessage());
            StringBuilder sb = new StringBuilder();
            sb.append("// ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append(" depth ").append(d).append("\n");
            sb.append("// callers: ");
            for (Function cf : f.getCallingFunctions(monitor)) sb.append(cf.getEntryPoint()).append(" ");
            sb.append("\n").append(c);
            try (PrintWriter pw = new PrintWriter(new File(outDir, f.getEntryPoint().toString() + ".c"))) {
                pw.print(sb);
            }
            if (d < depth) {
                for (Function cf : f.getCalledFunctions(monitor)) q.add(new Object[]{cf, d + 1});
            }
        }
        println("done, " + seen.size() + " functions");
    }
}
