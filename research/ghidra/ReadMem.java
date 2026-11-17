// Prints 32-bit words of the analysed image (method tables live in a section
// that is encrypted on disk, so they are read from the Ghidra project).
// args: outFile addr:count [addr:count ...]
// Each line: address, word, and what the word points at: "code" when it is an
// address inside an executable block, a string when Ghidra has one there.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import java.io.*;

public class ReadMem extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Memory mem = currentProgram.getMemory();
        try (PrintWriter pw = new PrintWriter(new File(a[0]))) {
            for (int i = 1; i < a.length; i++) {
                String[] p = a[i].split(":");
                long base = Long.parseLong(p[0].replace("0x", ""), 16);
                int n = p.length > 1 ? Integer.parseInt(p[1]) : 1;
                for (int k = 0; k < n; k++) {
                    Address x = toAddr(base + 4L * k);
                    long v;
                    try {
                        v = mem.getInt(x) & 0xffffffffL;
                    } catch (Exception e) {
                        pw.println(x + " ?");
                        continue;
                    }
                    String note = "";
                    Address t = toAddr(v);
                    MemoryBlock b = mem.getBlock(t);
                    if (b != null && b.isExecute()) {
                        Function f = getFunctionAt(t);
                        note = f != null ? "code fn" : "code";
                    } else if (b != null) {
                        Data d = getDataAt(t);
                        if (d != null && d.hasStringValue()) note = "str " + d.getValue();
                    }
                    pw.println(String.format("%08x %08x %s", x.getOffset(), v, note));
                }
                pw.println();
            }
        }
        println("done");
    }
}
