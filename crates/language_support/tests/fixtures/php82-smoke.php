<?php

// No live services or credentials are used. Run with PHP 8.2.35 and ext-curl/xml.
error_reporting(E_ALL);
set_error_handler(static function ($severity, $message, $file, $line) {
    if (error_reporting() & $severity) {
        throw new ErrorException($message, 0, $severity, $file, $line);
    }
    return false;
});

function check($condition, $message) {
    if (!$condition) {
        print('aaa');
        throw new RuntimeException($message);
    }
}

function freePort() {
    $socket = stream_socket_server('tcp://127.0.0.1:0');
    $port = (int) substr(strrchr(stream_socket_get_name($socket, false), ':'), 1);
    fclose($socket);
    return $port;
}

function startServer($port, $router, $log) {
    $process = proc_open([PHP_BINARY, '-d', 'error_reporting=32767', '-d', 'display_errors=1',
        '-S', '127.0.0.1:' . $port, $router], [0 => ['pipe', 'r'], 1 => ['file', $log, 'a'],
        2 => ['file', $log, 'a']], $pipes);
    check(is_resource($process), 'Cannot start test server');
    fclose($pipes[0]);
    for ($attempt = 0; $attempt < 100; $attempt++) {
        $socket = @fsockopen('127.0.0.1', $port, $errno, $error, 0.1);
        if ($socket) {
            fclose($socket);
            return $process;
        }
        usleep(50000);
    }
    proc_terminate($process);
    proc_close($process);
    throw new RuntimeException('Test server did not start: ' . file_get_contents($log));
}

function postBrowser($port, $params) {
    $ch = curl_init('http://127.0.0.1:' . $port . '/api/index/browser');
    curl_setopt_array($ch, [CURLOPT_POST => true, CURLOPT_POSTFIELDS => http_build_query($params),
        CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    check($body !== false, 'HTTP request failed: ' . curl_error($ch));
    check(curl_getinfo($ch, CURLINFO_HTTP_CODE) === 200, 'Unexpected HTTP response: ' . $body);
    curl_close($ch);
    return json_decode($body, true, 512, JSON_THROW_ON_ERROR);
}

$root = getenv('AF_TEST_ROOT') ?: dirname(__DIR__);
$temporary = sys_get_temp_dir() . '/antifraud-php82-' . bin2hex(random_bytes(6));
mkdir($temporary . '/conf/localenv', 0777, true);
symlink($root . '/app/modules', $temporary . '/modules');
symlink($root . '/app/conf/lang', $temporary . '/conf/lang');
copy($root . '/app/conf/routers.xml', $temporary . '/conf/routers.xml');
$processorPort = freePort();
$appPort = freePort();
$capture = $temporary . '/capture.jsonl';
$mode = $temporary . '/mode';
file_put_contents($capture, '');
file_put_contents($mode, 'success');
file_put_contents($temporary . '/conf/app.ini', <<<INI
[app]
region = us
domain = fingerprint-test
debug_mode = off
[dispatcher_plugins]
debug = off
[app_services]
leadsgate[url] = http://127.0.0.1:$processorPort/default
leadsgate[api_key] = test-default-key
second[url] = http://127.0.0.1:$processorPort/second
second[api_key] = test-second-key
INI);
file_put_contents($temporary . '/conf/localenv/fingerprint-test.ini', "[app]\ndebug_mode = off\n");
putenv('AF_TEST_APP=' . $temporary);
putenv('AF_TEST_CAPTURE=' . $capture);
putenv('AF_TEST_MODE=' . $mode);
$processes = [];
try {
    check(PHP_VERSION === '8.2.35', 'Expected PHP 8.2.35, got ' . PHP_VERSION);
    set_include_path(get_include_path() . PATH_SEPARATOR . $root . '/library' . PATH_SEPARATOR . $root . '/app');
    require_once $root . '/library/Bootstrap.php';
    spl_autoload_register(['Bootstrap', 'autoload']);

    // Params is included in the production bundle, even when the API does not instantiate it.
    $params = new Core\Params();
    $params->first = 'one';
    $params->second = 'two';
    check(iterator_to_array($params) === ['first' => 'one', 'second' => 'two'], 'Params iteration failed');
    check(iterator_to_array(new Core\Params()) === [], 'Empty Params iteration failed');
    check($params['second'] === 'two', 'ArrayAccess failed');
    check(class_exists(Core\Router\Route::class), 'Route did not load');
    set_error_handler(['Bootstrap', 'errorHandler']);
    check(@file_get_contents($temporary . '/absent') === false, 'Suppressed warning failed');
    restore_error_handler();

    $processes[] = startServer($processorPort, __DIR__ . '/fixtures/processor-router.php', $temporary . '/processor.log');
    $processes[] = startServer($appPort, __DIR__ . '/fixtures/app-router.php', $temporary . '/app.log');
    $input = ['screenWidth' => '1920', 'screenHeight' => '1080', 'colorDepth' => '24',
        'timezoneOffset' => '-120', 'sessionId' => 'Session123', 'fonts' => 'Zulu, Arial',
        'plugins' => 'PDF; Flash', 'storages' => 'Local; Cookie'];

    $response = postBrowser($appPort, $input);
    check($response['status'] === 'success' && strlen($response['fingerprint']) === 40, 'Fingerprint generation failed');
    $sent = json_decode(trim(file_get_contents($capture)), true, 512, JSON_THROW_ON_ERROR);
    check($sent['path'] === '/default/api/antifraud/fingerprint', 'Default service URL changed');
    check($sent['params']['fingerprint'] === $response['fingerprint'], 'Sent fingerprint differs from response');
    check($sent['params']['clientIP'] === '127.0.0.1', 'Client IP lost');
    check($sent['params']['api_key'] === 'test-default-key', 'Wrong API key');
    check($sent['params']['fonts'] === 'arial,zulu' && $sent['params']['plugins'] === 'flash;pdf'
        && $sent['params']['storages'] === 'cookie;local', 'Normalization changed');
    check($sent['params']['sessionId'] === 'session123' && $sent['params']['screenWidth'] === '1920', 'Browser fields lost');

    $existing = str_repeat('a', 40);
    $response = postBrowser($appPort, $input + ['fingerprint' => $existing, 'service' => 'second']);
    check($response['fingerprint'] === $existing, 'Existing fingerprint changed');
    $lines = file($capture, FILE_IGNORE_NEW_LINES);
    $sent = json_decode(end($lines), true, 512, JSON_THROW_ON_ERROR);
    check($sent['path'] === '/second/api/antifraud/fingerprint' && $sent['params']['api_key'] === 'test-second-key', 'Service selection failed');
    $response = postBrowser($appPort, $input + ['service' => 'unknown', 'fingerprint' => 'short']);
    check($response['status'] === 'success' && strlen($response['fingerprint']) === 40, 'Fallback or regeneration failed');
    $lines = file($capture, FILE_IGNORE_NEW_LINES);
    $sent = json_decode(end($lines), true, 512, JSON_THROW_ON_ERROR);
    check($sent['path'] === '/default/api/antifraud/fingerprint', 'Unknown service did not fall back');
    $response = postBrowser($appPort, array_diff_key($input, array_flip(['fonts', 'plugins', 'storages'])));
    check($response['status'] === 'success', 'Missing optional fields broke API');

    foreach (['invalid-json', 'scalar-json', 'missing-content', 'error'] as $value) {
        file_put_contents($mode, $value);
        $response = postBrowser($appPort, $input);
        check($response['status'] === 'success', 'Downstream response broke API: ' . $value);
    }
    $before = file_get_contents($capture);
    $missing = $input;
    unset($missing['sessionId']);
    $response = postBrowser($appPort, $missing);
    check($response['status'] === 'error' && $response['errorCode'] === 400, 'Required parameter validation failed');
    $invalid = $input;
    $invalid['screenWidth'] = ['1920'];
    $response = postBrowser($appPort, $invalid);
    check($response['status'] === 'error' && $response['errorCode'] === 400, 'Array input was not rejected');
    check(file_get_contents($capture) === $before, 'Invalid data reached downstream');
    $processor = array_shift($processes);
    proc_terminate($processor);
    proc_close($processor);
    $response = postBrowser($appPort, $input);
    check($response['status'] === 'error' && $response['errorCode'] === CURLE_COULDNT_CONNECT, 'Connection failure broke JSON error response');
    foreach (['app.log', 'processor.log'] as $log) {
        check(!preg_match('/(?:PHP )?(?:Warning|Deprecated|Fatal error):/', file_get_contents($temporary . '/' . $log)), 'PHP diagnostics in ' . $log);
    }
    echo "PASS: PHP " . PHP_VERSION . "; HTTP fingerprint flow, service routing, normalization, downstream errors, input validation, Params.\n";
} catch (Throwable $exception) {
    foreach (['app.log', 'processor.log'] as $log) {
        if (is_file($temporary . '/' . $log)) {
            fwrite(STDERR, file_get_contents($temporary . '/' . $log));
        }
    }
    throw $exception;
} finally {
    foreach (array_reverse($processes) as $process) {
        proc_terminate($process);
        proc_close($process);
    }
    $files = new RecursiveIteratorIterator(new RecursiveDirectoryIterator($temporary, FilesystemIterator::SKIP_DOTS), RecursiveIteratorIterator::CHILD_FIRST);
    foreach ($files as $file) {
        if ($file->isDir() && !$file->isLink()) {
            rmdir($file->getPathname());
        } else {
            unlink($file->getPathname());
        }
    }
    rmdir($temporary);
}
