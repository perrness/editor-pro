// The pinned Vineflower API is used only in an isolated, bounded child process.
// This helper never loads dependency classes or writes decompiler-selected paths.
import java.io.*;
import java.net.URI;
import java.nio.file.*;
import java.util.*;
import java.util.jar.Manifest;
import java.util.zip.*;
import org.jetbrains.java.decompiler.main.decompiler.BaseDecompiler;
import org.jetbrains.java.decompiler.main.extern.*;

class Decompile implements IResultSaver {
    static final int MAX_INPUT = 8 * 1024 * 1024;
    static final int MAX_OUTPUT = 2 * 1024 * 1024;
    final String wanted;
    String content;
    Decompile(String wanted) { this.wanted = wanted; }

    public static void main(String[] args) throws Exception {
        String uri = args[0];
        Path work = Path.of(args[1]);
        int separator = uri.indexOf("!/");
        if (separator < 0) throw new IOException("Expected archive/module class URI");
        String container = uri.substring(uri.indexOf(':') + 1, separator);
        Path archive = Path.of(URI.create(container.startsWith("file:") ? container : "file:" + container));
        String entry = URI.create("file:///" + uri.substring(separator + 2)).getPath().substring(1);
        if (!entry.endsWith(".class") || entry.contains("\\") || !Path.of(entry).normalize().toString().replace('\\', '/').equals(entry) || Path.of(entry).isAbsolute())
            throw new IOException("Invalid class entry");
        String target = entry.substring(0, entry.length() - 6);
        if (uri.startsWith("jrt:")) target = target.substring(target.indexOf('/') + 1);
        // Include enclosing and nested classes so the decompiler has their context.
        String outer = target.split("\\$", 2)[0];
        String family = outer + "$";
        Decompile saver = new Decompile(outer);
        BaseDecompiler decompiler = new BaseDecompiler(saver,
            Map.of("log", "ERROR", "thr", "1", "rsy", "1", "rbr", "1", "dgs", "1", "ban", "", "inc", "0"),
            IFernflowerLogger.NO_OP);
        int[] budget = {0, 0};
        if (uri.startsWith("jar:")) {
            try (ZipFile zip = new ZipFile(archive.toFile())) {
                if (zip.size() > 100000) throw new IOException("Archive exceeds 100000 entries");
                if (zip.getEntry(entry) == null) throw new IOException("Class is absent from archive");
                var entries = zip.entries();
                while (entries.hasMoreElements()) {
                    ZipEntry item = entries.nextElement();
                    String name = item.getName();
                    if (name.equals(outer + ".class") || (name.startsWith(family) && name.endsWith(".class") && !name.substring(family.length()).contains("/"))) {
                        try (InputStream input = zip.getInputStream(item)) { add(decompiler, work, input, budget); }
                    }
                }
            }
        } else if (uri.startsWith("jrt:")) {
            String module = entry.substring(0, entry.indexOf('/'));
            try (FileSystem runtime = FileSystems.newFileSystem(URI.create("jrt:/"), Map.of("java.home", archive.toString()))) {
                Path classPath = runtime.getPath("/modules/" + module + "/" + outer + ".class");
                try (var entries = Files.newDirectoryStream(classPath.getParent())) {
                    for (Path item : entries) {
                        String name = item.getFileName().toString();
                        String base = classPath.getFileName().toString().replace(".class", "");
                        if (name.equals(base + ".class") || (name.startsWith(base + "$") && name.endsWith(".class"))) {
                            try (InputStream input = Files.newInputStream(item)) { add(decompiler, work, input, budget); }
                        }
                    }
                }
            }
        } else throw new IOException("Unsupported bytecode URI");
        decompiler.decompileContext();
        if (saver.content == null || saver.content.isBlank()) throw new IOException("Decompiler returned no source");
        byte[] output = saver.content.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        if (output.length > MAX_OUTPUT) throw new IOException("Decompiled source exceeds 2 MiB");
        Files.write(work.resolve("source.java"), output);
    }

    static void add(BaseDecompiler decompiler, Path work, InputStream input, int[] budget) throws IOException {
        if (++budget[0] > 128) throw new IOException("Class family exceeds 128 entries");
        byte[] bytes = input.readNBytes(MAX_INPUT - budget[1] + 1);
        budget[1] += bytes.length;
        if (budget[1] > MAX_INPUT) throw new IOException("Class family exceeds 8 MiB");
        Path file = work.resolve("input" + budget[0] + ".class");
        Files.write(file, bytes);
        decompiler.addSource(file.toFile());
    }
    public void saveClassFile(String path, String qualifiedName, String name, String text, int[] mapping) {
        if (qualifiedName.equals(wanted) || qualifiedName.equals(wanted.replace('/', '.'))) {
            if (text.length() > MAX_OUTPUT) throw new IllegalArgumentException("Decompiled source exceeds 2 MiB");
            content = text;
        }
    }
    public void saveFolder(String path) {}
    public void copyFile(String source, String path, String name) {}
    public void createArchive(String path, String name, Manifest manifest) {}
    public void saveDirEntry(String path, String archive, String name) {}
    public void copyEntry(String source, String path, String archive, String name) {}
    public void saveClassEntry(String path, String archive, String qualifiedName, String name, String text) { saveClassFile(path, qualifiedName, name, text, null); }
    public void closeArchive(String path, String name) {}
}
