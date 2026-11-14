/* Compiles one HLSL shader to Direct3D 9 bytecode with D3DCompile of
 * d3dcompiler_47.dll (Windows, or Wine with that DLL next to this program).
 * Used by tools/build-render-shaders.sh for the graphics module (stage 59).
 *
 *   compile.exe SOURCE.hlsl OUT.cso ENTRY TARGET    e.g. main ps_3_0
 *
 * Copyright (C) the doctor_toolkit authors. GPL-3.0-only.
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

typedef HRESULT(WINAPI *compile_fn)(const void *data, SIZE_T size, const char *name,
                                    const void *defines, void *include, const char *entry,
                                    const char *target, UINT flags1, UINT flags2,
                                    void **code, void **errors);

/* ID3DBlob: IUnknown, then GetBufferPointer, GetBufferSize. */
typedef void *(WINAPI *blob_pointer_fn)(void *self);
typedef SIZE_T(WINAPI *blob_size_fn)(void *self);

static void *blob_pointer(void *blob) {
    void **vtbl = *(void ***)blob;
    return ((blob_pointer_fn)vtbl[3])(blob);
}

static SIZE_T blob_size(void *blob) {
    void **vtbl = *(void ***)blob;
    return ((blob_size_fn)vtbl[4])(blob);
}

int main(int argc, char **argv) {
    if (argc != 5) {
        fprintf(stderr, "usage: compile SOURCE OUT ENTRY TARGET\n");
        return 2;
    }
    HMODULE lib = LoadLibraryA("d3dcompiler_47.dll");
    if (!lib) {
        fprintf(stderr, "cannot load d3dcompiler_47.dll\n");
        return 1;
    }
    compile_fn compile = (compile_fn)GetProcAddress(lib, "D3DCompile");
    if (!compile) {
        fprintf(stderr, "no D3DCompile in d3dcompiler_47.dll\n");
        return 1;
    }
    FILE *in = fopen(argv[1], "rb");
    if (!in) {
        fprintf(stderr, "cannot open %s\n", argv[1]);
        return 1;
    }
    fseek(in, 0, SEEK_END);
    long size = ftell(in);
    fseek(in, 0, SEEK_SET);
    char *text = malloc(size + 1);
    if (!text || fread(text, 1, size, in) != (size_t)size) {
        fprintf(stderr, "cannot read %s\n", argv[1]);
        return 1;
    }
    fclose(in);
    void *code = NULL, *errors = NULL;
    /* D3DCOMPILE_OPTIMIZATION_LEVEL3 */
    HRESULT hr = compile(text, size, argv[1], NULL, NULL, argv[3], argv[4], 1u << 15, 0,
                         &code, &errors);
    if (errors) {
        fwrite(blob_pointer(errors), 1, blob_size(errors), stderr);
    }
    if (FAILED(hr) || !code) {
        fprintf(stderr, "D3DCompile failed: 0x%08lx\n", (unsigned long)hr);
        return 1;
    }
    FILE *out = fopen(argv[2], "wb");
    if (!out || fwrite(blob_pointer(code), 1, blob_size(code), out) != blob_size(code)) {
        fprintf(stderr, "cannot write %s\n", argv[2]);
        return 1;
    }
    fclose(out);
    return 0;
}
