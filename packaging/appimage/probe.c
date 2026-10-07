/* Non-GUI runtime checks. Never calls gtk_init() or adw_init(). */
#include <gtk/gtk.h>
#include <adwaita.h>
#include <turbojpeg.h>
#include <webp/encode.h>
#include <fontconfig/fontconfig.h>
#include <stdio.h>

static void fail(const char *message) { fprintf(stderr, "%s\n", message); exit(1); }
static void css_error(GtkCssProvider *provider, GtkCssSection *section, GError *error, gpointer data) {
    (void)provider; (void)section; (void)data;
    fail(error->message);
}
static void check_fonts(void) {
    if (FcGetVersion() != 21500) fail("Fontconfig is not 2.15.0");
    FcConfig *config = FcInitLoadConfigAndFonts();
    if (!config) fail("Fontconfig initialization failed");
    FcStrList *files = FcConfigGetConfigFiles(config);
    FcChar8 *file;
    const char *base = g_getenv("FONTCONFIG_PATH");
    while ((file = FcStrListNext(files))) {
        printf("Fontconfig config: %s\n", file);
        if (g_str_has_prefix((char *)file, "/etc/fonts/") ||
            g_str_has_prefix((char *)file, "/usr/share/fontconfig/")) fail("Host system font rules leaked");
    }
    FcStrListDone(files);
    if (!base) fail("FONTCONFIG_PATH missing");
    FcChar8 *templates = FcConfigFilename((FcChar8 *)"conf.avail");
    char *expected = g_build_filename(base, "conf.avail", NULL);
    if (g_strcmp0((char *)templates, expected)) fail("Fontconfig template directory outside AppDir");
    FcStrFree(templates); g_free(expected);
    FcFontSet *fonts = FcConfigGetFonts(config, FcSetSystem);
    char *user_fonts = g_build_filename(g_getenv("XDG_DATA_HOME"), "fonts", NULL);
    gboolean user_found = FALSE;
    for (int i=0; fonts && i<fonts->nfont; i++) {
        if (FcPatternGetString(fonts->fonts[i], FC_FILE, 0, &file) == FcResultMatch &&
            g_str_has_prefix((char *)file, user_fonts)) user_found = TRUE;
    }
    g_free(user_fonts);
    if (!user_found) fail("XDG user font not discovered");
    puts("Fontconfig XDG user font: discovered");
    const char *patterns[] = {"sans-serif:lang=ja:charset=65e5", "emoji:charset=1f600", "monospace"};
    for (int i=0; i<3; i++) {
        FcPattern *pattern = FcNameParse((FcChar8 *)patterns[i]);
        if (!pattern || !FcConfigSubstitute(config, pattern, FcMatchPattern)) fail("Font pattern failed");
        FcDefaultSubstitute(pattern);
        FcResult result;
        FcPattern *match = FcFontMatch(config, pattern, &result);
        if (!match || FcPatternGetString(match, FC_FILE, 0, &file) != FcResultMatch) fail("Font match missing");
        printf("Fontconfig match %s: %s\n", patterns[i], file);
        FcPatternDestroy(match); FcPatternDestroy(pattern);
    }
    FcConfigDestroy(config);
}
int main(int argc, char **argv) {
    if (argc == 3 && g_str_equal(argv[1], "--make-fixtures")) {
        g_mkdir_with_parents(argv[2], 0755);
        GdkPixbuf *p = gdk_pixbuf_new(GDK_COLORSPACE_RGB, FALSE, 8, 32, 16);
        gdk_pixbuf_fill(p, 0x3579bdff);
        const char *types[] = {"jpeg", "png"};
        for (int i=0; i<2; i++) {
            char *path = g_strdup_printf("%s/test.%s", argv[2], types[i]);
            GError *error = NULL;
            if (!gdk_pixbuf_save(p, path, types[i], &error, NULL)) fail(error->message);
            g_free(path);
        }
        uint8_t *data = NULL;
        size_t len = WebPEncodeLosslessRGB(gdk_pixbuf_get_pixels(p), 32, 16,
                                          gdk_pixbuf_get_rowstride(p), &data);
        char *path = g_build_filename(argv[2], "test.webp", NULL);
        if (!len || !g_file_set_contents(path, (char *)data, len, NULL)) fail("WebP fixture failed");
        WebPFree(data); g_free(path); g_object_unref(p); return 0;
    }
    printf("GTK %u.%u.%u; Adwaita %u.%u.%u; Cairo %s; TurboJPEG %d\n",
           gtk_get_major_version(), gtk_get_minor_version(), gtk_get_micro_version(),
           adw_get_major_version(), adw_get_minor_version(), adw_get_micro_version(),
           cairo_version_string(), TURBOJPEG_VERSION_NUMBER);
    if (TURBOJPEG_VERSION_NUMBER != 3002000) fail("TurboJPEG headers are not 3.2.0");
    if (g_strcmp0(cairo_version_string(), "1.18.6")) fail("Cairo runtime is not 1.18.6");
    if (gtk_get_major_version() != 4 || gtk_get_minor_version() != 14) fail("CSS must be validated with GTK 4.14");
    if (argc < 3 || !g_str_equal(argv[1], "--css")) fail("Actual patched APP_CSS input missing");
    GtkCssProvider *css = gtk_css_provider_new();
    g_signal_connect(css, "parsing-error", G_CALLBACK(css_error), NULL);
    const char *variants[] = {"light", "dark"};
    for (int i=0; i<2; i++) {
        char *resource = g_strdup_printf("/org/gnome/Adwaita/styles/defaults-%s.css", variants[i]);
        GBytes *defaults = g_resources_lookup_data(resource, 0, NULL);
        if (!defaults) fail("Adwaita color definitions missing");
        gsize length;
        const char *data = g_bytes_get_data(defaults, &length);
        const char *colors[] = {"@define-color sidebar_bg_color ", "@define-color sidebar_fg_color ",
                                "@define-color sidebar_backdrop_color "};
        for (int j=0; j<3; j++) if (!g_strstr_len(data, length, colors[j])) fail("Adwaita sidebar color missing");
        gtk_css_provider_load_from_resource(css, resource);
        printf("Adwaita %s sidebar/backdrop colors: present, zero parsing errors\n", variants[i]);
        g_bytes_unref(defaults); g_free(resource);
    }
    gtk_css_provider_load_from_path(css, argv[2]);
    g_object_unref(css);
    puts("Patched APP_CSS: zero parsing errors");
    check_fonts();
    tjhandle h = tj3Init(TJINIT_DECOMPRESS);
    if (!h) fail("TurboJPEG 3 API missing");
    tj3Destroy(h);
    const char *schemas[] = {"org.gtk.gtk4.Settings.FileChooser", "org.gnome.desktop.interface"};
    GSettingsSchemaSource *source = g_settings_schema_source_get_default();
    if (!source) fail("GSettings source missing");
    for (int i=0; i<2; i++) {
        GSettingsSchema *s = g_settings_schema_source_lookup(source, schemas[i], TRUE);
        if (!s) fail(schemas[i]);
        g_settings_schema_unref(s);
    }
    GError *error = NULL;
    char **gtk_resources = g_resources_enumerate_children("/org/gtk/libgtk/", 0, &error);
    if (!gtk_resources) fail(error->message);
    g_strfreev(gtk_resources);
    char **adw_resources = g_resources_enumerate_children("/org/gnome/Adwaita/", 0, &error);
    if (!adw_resources) fail(error->message);
    g_strfreev(adw_resources);
    GSettings *settings = g_settings_new("org.gnome.desktop.interface");
    char *font = g_settings_get_string(settings, "font-name");
    g_free(font); g_object_unref(settings);
    GList *modules = g_io_modules_load_all_in_directory(g_getenv("GIO_MODULE_DIR"));
    if (!modules) fail("GIO dconf module missing");
    g_list_free(modules);
    for (int i=3; i<argc; i++) {
        GdkTexture *t = gdk_texture_new_from_filename(argv[i], &error);
        if (!t) fail(error->message);
        printf("GDK decode %s: %dx%d\n", argv[i], gdk_texture_get_width(t), gdk_texture_get_height(t));
        g_object_unref(t);
        GdkPixbuf *p = gdk_pixbuf_new_from_file(argv[i], &error);
        if (!p) fail(error->message);
        printf("Pixbuf decode %s: %dx%d\n", argv[i], gdk_pixbuf_get_width(p), gdk_pixbuf_get_height(p));
        g_object_unref(p);
    }
    return 0;
}
