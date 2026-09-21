# Plano de design (rascunho 1, antes da pesquisa de concorrentes)

## O briefing

**Assunto**: saber se o seu ônibus está vindo, no Rio.
**Quem usa**: passageiro do Rio, em pé no ponto, uma mão no celular, sol na tela, Android intermediário, 4G instável, plano de dados contado.
**A tarefa**: responder "dá tempo?" em um relance de três segundos.
**O que temos hoje**: posição ao vivo de ~2.000 ônibus com sentido confirmado, distância ao longo do traçado, próxima parada, idade do dado por veículo e estado (em viagem, no terminal, sentido não confirmado, sem sinal, estacionado). Chegadas em minutos só na Fase 3, e sempre como faixa.

## O que é característico deste mundo

Ninguém procura "um ônibus". Procura **um número**, lido de longe no letreiro da frente. É isso que o rosto do app tem que ser.

Três coisas específicas do Rio que a concorrência não trata bem e que já estão nos nossos dados:

1. **O letreiro.** O número da linha é a informação, não um rótulo ao lado dela.
2. **O BRS.** Nomes de parada como `BRS 1,2,3,4,5,I: Erasmo Braga` dizem em qual faixa numerada do corredor você tem que ficar. Hoje isso aparece como lixo no meio do nome. Separado, vira instrução: "Erasmo Braga · faixas BRS 1 a 5 e I".
3. **A honestidade.** A queixa número um sobre os concorrentes é a previsão que pula de "10 minutos" para "já passou". Enquanto não temos histórico, a resposta certa não é um minuto inventado: é **quantas paradas faltam**, que é verificável pela janela.

## Sistema

### Cor

Uma ideia só: a tela é feita de **placas de letreiro** sobre um campo claro.

| Token | Valor | Uso |
|---|---|---|
| `--placa` | `#000000` | a placa do letreiro; preto real, contraste máximo sob sol |
| `--campo` | `#FFFFFF` | fundo |
| `--campo-2` | `#EDEFF2` | agrupamento, sem caixas nem sombras |
| `--vivo` | `#FFC400` | âmbar de LED. **Só** para "este ônibus está andando agora" |
| `--passou` | `#FF5A36` | já passou, sem sinal, erro |
| `--apoio` | `#5A6470` | texto secundário |

Modo escuro inverte campo e placa; o âmbar não muda. A ousadia está gasta num lugar só: o âmbar. Tudo o mais é preto e branco.

Nada de creme com terracota, nada de preto com verde ácido, nada de cartões arredondados iguais com sombra cinza.

### Tipografia

**Archivo Variable** (eixos de peso e largura), uma família com duas vozes:
- números de linha: largura estreita, peso 700, como um letreiro
- texto: largura normal, pesos 400 e 500

Um arquivo, subconjunto latino, woff2, cache de um ano. Numerais tabulares em toda parte, porque a tela é feita de números que mudam.

### Layout

Lista primeiro, mapa depois. Quem está no ponto quer uma lista; o mapa é a segunda tela.

```
 Perto de você                      ⟳ há 8 s
 ─────────────────────────────────────────────
 Debret                             123 m  ↑
 ███ 201   Castelo         agora no ponto
 ███ 202   Castelo         8 paradas · 2,5 km
 ███ 232   Castelo         7 paradas · 2,8 km

 Erasmo Braga · BRS 1 a 5, I        195 m  ↑
 ███ 455   Méier           2 paradas · 837 m
 ███ 384   Pavuna          2 paradas · 914 m
```

A placa preta com o número à esquerda é o herói. À direita, a resposta: o número de paradas em destaque, a distância como apoio. Alinhamento à esquerda; os números de paradas alinhados numa coluna própria para dar para varrer com o olho.

### Princípios

1. **O número da linha é o herói.** Tudo o mais é apoio.
2. **Não invente minutos.** Paradas e metros são verificáveis; minuto sem histórico é chute.
3. **A idade do dado é parte da resposta**, não uma nota de rodapé.
4. **Uma cor só carrega significado.** Âmbar quer dizer "vivo agora".
5. **Nada se mexe sem motivo.** O único movimento é o ônibus andando no mapa e a contagem de idade.

## A revisar quando a pesquisa chegar

- Lista primeiro contra mapa primeiro: confirmar com o que os melhores fazem e com o que os usuários reclamam.
- Formato da resposta: paradas, distância, minutos ou combinação.
- Onde entram favoritos e busca.
- Se "placa preta" se sustenta com 20 linhas na tela ou vira parede preta.

---

# Revisão 2, depois da pesquisa (21/09/2026)

Duas pesquisas: apps brasileiros (lidos direto das telas das lojas e das avaliações) e os melhores do mundo (lidos do código-fonte dos clientes abertos e dos artigos dos próprios times). O que elas mudaram.

## Confirmado

- **Abrir já com a resposta na tela, sem pedir nada.** É o modelo do Transit e é o que a avaliação mais votada do Lá vem o ônibus elogia: "você abre e já tem um mapa com pontos de ônibus próximos". Nada de login, nada de onboarding.
- **Idade do dado por veículo, em palavras.** O Lá vem mostra "1:18 atrás", o OneBusAway "Data updated 45 sec ago", o Transit põe no canto do ícone. Numa medição de uma frota real, 39% dos ônibus desenhados no mapa não reportavam havia mais de um minuto. Não é caso raro, é o caso comum.
- **Distância e paradas quando o minuto seria mentira.** É a escolha declarada do MTA Bus Time: "começamos pelo que sabemos com certeza, onde o ônibus está agora".
- **Não interpolar a posição.** O bustimes.org, que desenha mais ônibus que qualquer um, deixa a posição pular de propósito. Animar inventa dado e gasta bateria justamente com a tela ligada no ponto.

## Corrigido por causa da pesquisa

| O que mudou | Por quê |
|---|---|
| Correção de desvio de relógio | "GPS há 40 s" é mentira se o relógio do aparelho está errado, e em Android intermediário isso é comum. Agora o cabeçalho `Date` de cada resposta ajusta a conta. Vinte linhas, a credibilidade mais barata do relatório |
| Escada de distância com os limiares publicados | No ponto até 30 m, chegando até 150 m, paradas até 4, distância além. São os números do MTA Bus Time, não chutes meus |
| Ônibus de perto desenhados no mapa | O mapa estava decorativo na tela inicial. Agora mostra os ônibus de que a lista está falando, que é a única razão para ele estar ali |
| Degradação aos 90 s | É o limite que o guia do GTFS-Realtime trata como "atual". Aos 90 s a linha perde confiança visual, aos 180 s vira "sem sinal" |
| Placa que encolhe com o código | De "3" a "LECD133". A fonte estreita e diminui em vez de cortar, como o Digitransit faz nos marcadores |
| Altura de linha mínima de 58 px | Alvo de toque confortável; a queixa mais votada do líder de mercado é "os ícones são muito pequenos, quem usa esse app está na rua, se movimentando" |

## Guardado para a Fase 3

- **A faixa e o número têm que ocupar o mesmo lugar.** O estudo do CHI 2016 mostrou que um ponto em destaque longe da incerteza faz o leitor ignorar a incerteza. "4 a 7 min" é um objeto tipográfico só, não um número grande com uma nota ao lado.
- **Largura da faixa por horizonte**, das bandas que o Transit publica: 10 a 15 min tolera 1,5 min adiantado e 4,5 min atrasado; 6 a 10 min, 1,0 e 3,5; 3 a 6 min, 1,0 e 2,5; 0 a 3 min, 30 s e 1,5 min. Assimétrico de propósito: adiantar faz perder o ônibus, atrasar só faz esperar.
- **Escolher um nível de confiança e não mudar.** O CHI 2018 mediu que faixas de 60% e de 99% funcionam e a de 85% não funciona melhor que nada. Nossos p20/p80 dão 60%, que é defensável. Dizer isso uma vez na legenda.
- **Penalizar previsão que treme.** O Transit suaviza explicitamente ETAs que pulam de 5 para 3 para 8.
- **"Passou há 2 min" como estado de primeira classe.** O OneBusAway mantém o ônibus na lista com minutos negativos. Hoje nossas chegadas só olham para a frente; ver que o ônibus já passou é o que evita esperar em vão.
- **Cor semântica em três camadas de contraste**, do OneBusAway: uma para o número grande (3:1), uma para texto pequeno (4,5:1) e uma para texto branco sobre a cor (4,8:1), com o desvio nunca tingido pela marca.

## Adendo da pesquisa de sistemas de design de agências

- **Nenhum prêmio de design para app de transporte desde 2023.** Seis buscas independentes em D&AD, Red Dot, iF, Webby, Fast Company e UX Design Awards não acharam um só. O que ganhou prêmio é sinalização física e é antiga (Legible London, 2008 a 2010). Não existe referência consagrada a copiar: o campo está aberto.
- **Gerar a paleta, nunca digitar hex.** O padrão de cor da própria TfL, obrigatório e de 12 páginas, se contradiz em dois Pantones: o verde corporativo é (0,121,52) numa página e (0,125,50) na outra. Se a marca de transporte mais disciplinada do mundo não mantém um documento coerente, a nossa paleta sai de uma tabela gerada e verificada, que é o que passou a ser feito: oito matizes escolhidos por busca, dE mínimo de 45,8 entre qualquer par, contraste mínimo de 3,5:1 contra o próprio fundo, um conjunto por tema.
- **Tempo a pé, não só distância.** Os 4,5 km/h são a velocidade para a qual as placas do Legible London foram calibradas, e os raios de 375 m (5 min) e 1.125 m (15 min) são a base delas.
- **48 px como tamanho mínimo de alvo**, que é o que os tokens publicados da SBB usam.
- **A fase vai no payload.** A API de Singapura carrega um campo `Monitored` 0 ou 1 dizendo se a estimativa é de GPS ou de tabela. Nosso `ph` faz o mesmo. O contraexemplo é a MBTA, que publica guia de exibição em tempo real e não especifica nenhuma diferença visual entre previsto e programado.
- **Não lançar lotação na v1.** Seul modela a partir de dados de cartão, a Moovit depende de colaboração dos usuários, a MBTA não publica. É passivo de credibilidade antes de existir base de usuários.
- **Ideia guardada da Deutsche Bahn**: uma escala única de oito degraus compartilhada por espaçamento, tamanho e tipografia, e três modos de densidade trocáveis por token. Faz sentido quando a lista tiver mais tipos de linha.
- **`light-dark()` do CSS foi descartado de propósito**: resolve a duplicação de tokens em uma linha, mas só existe em navegadores de 2024 em diante, e o público deste app é justamente quem tem WebView antigo.

---

# Revisão 3: a tela é sobre as suas linhas (21/09/2026)

A versão anterior listava tudo o que passava perto: todas as linhas de todas as paradas num raio. O Brian apontou o erro central: ninguém quer todos os ônibus. O uso real é outro:

1. escolher as poucas linhas que você pega;
2. ver a rota colorida delas;
3. ver os pontos delas;
4. saber quanto vai demorar.

É o modelo que a pesquisa já mostrava como o mais elogiado do líder de mercado (linhas escolhidas, uma cor por linha) e eu não tinha seguido.

## O que mudou

- **Tela inicial = suas linhas.** Sem nenhuma escolhida, abre direto na escolha: busca por número ou destino, e as linhas que passam a menos de 500 m, cada uma com os dois destinos e a distância a pé. A escolha fica salva no aparelho, até oito linhas, e cada uma sai com um toque no ×.
- **Um cartão por linha, um bloco por sentido.** Para cada sentido, a parada mais próxima de você com o tempo a pé, e a faixa de tempo dos próximos ônibus até ela.
- **Mapa só com as suas linhas.** Rota na cor da linha, paradas no traçado, a parada que você usaria em destaque, os ônibus vindo. Tocar no cartão foca a linha e apaga as outras. O mapa se enquadra sozinho em você, nas paradas e nos ônibus a caminho.
- **Cores distintas por ordem de escolha.** Duas linhas que você acompanha juntas nunca dividem cor.
- **Tempo, finalmente.** "1 a 5 min", como um objeto tipográfico só.

## Como o tempo é calculado, e por que não pelo horário oficial

Os horários do GTFS são interpolados a velocidade constante em 2.997 de 3.000 viagens: eles só sabem a média da linha, nada sobre onde ela é lenta. A média planejada é de 24 km/h; os ônibus do Centro andam, medidos agora, de 7 a 12 km/h. Usar o horário daria tempos otimistas no Centro, exatamente o "falta 5 minutos há 45 minutos" das avaliações.

A estimativa usa três fontes, pesadas pela distância até a parada:
- a velocidade do próprio ônibus, que vale para o próximo quilômetro (peso que cai com meia distância de 1,5 km);
- a mediana da velocidade dos outros ônibus da mesma linha agora, que é o trânsito real do corredor;
- a velocidade planejada, só quando há menos de três ônibus da linha rodando.

A faixa vai de 0,7 a 1,4 vez o tempo central, mais meio minuto, com piso de 8 e teto de 40 km/h. Assimétrica porque o ônibus que chega antes do previsto é o que você perde. Fica calibrada de verdade na Fase 3, com o histórico por trecho.

---

# Revisão 4: imitar o Lá vem o ônibus (21/09/2026)

Pedido do Brian: a interface estava diferente demais dos concorrentes, e ele quer imitar o Lá vem o ônibus. O argumento é familiaridade: quem pega ônibus no Rio já sabe usar o Lá vem, e uma tela diferente cobra um reaprendizado que ninguém pediu. A linguagem das placas de letreiro (revisões 1 a 3) foi abandonada.

## Copiado do Lá vem, a partir das telas das lojas

- Mapa em tela cheia, sem barra de abas.
- Busca em pílula branca no topo: ícone de ônibus, "Qual linha procura?", "Rio de Janeiro" à direita e lupa verde redonda. Ao tocar, vira seta de voltar e abre uma lista em cartão com estrela, selo azul-marinho e "Origem x Destino".
- Linhas acompanhadas como bolinhas coloridas no canto esquerdo, restauradas ao abrir.
- Engrenagem branca no alto à direita; botão verde de busca e botão branco de localização embaixo à direita.
- Pílula branca embaixo com ícone de ônibus em círculo com anel, "Linha 202" com o sinal verde de tempo real, e o tempo em negrito.
- Folha "HORÁRIOS DOS ÔNIBUS": ícone azul-marinho da estação, nome da parada, abas "Tempo real" e "Tabela de horários", linhas com selo azul-marinho, destino em negrito, número grande com "min" pequeno e o sinal verde, "depois · N min" embaixo e a nota de rodapé.
- Balão do ônibus: bloco na cor da linha com o número e o prefixo do carro, bloco branco com "52 seg atrás" e a velocidade.
- Ajustes em tela cheia com chaves: modo escuro, exibir rota, exibir paradas, exibir ônibus sem sinal.
- Fonte do sistema (Roboto no Android), verde #2E7D32, azul-marinho #303F9F, raios de 24 px na busca, 16 px nos cartões e 20 px na folha. A fonte própria de 44 KB saiu.

## Mantido diferente, só onde os usuários do Lá vem reclamam

| Queixa nas avaliações do Lá vem | O que fazemos |
|---|---|
| "difícil saber qual sentido estamos acompanhando" | O sentido fica escrito na pílula ("Sentido Castelo · Debret") e trocar de sentido é uma opção visível no menu ⋮ |
| "tudo aumenta de tamanho, menos os ônibus" | Ícones de ônibus e de parada crescem com o zoom, e o ônibus tem uma seta de direção |
| "não consigo remover só um" favorito | Cada linha sai sozinha, pela estrela ou pelo menu da pílula |
| a parada só mostra as linhas favoritadas | A folha da parada mostra todas as linhas que passam ali |
| "está em 4 minutos há 45 minutos" | Tempo em faixa ("em 6 a 15 min"), com a folha de horários mostrando o número grande no mesmo lugar em que o Lá vem mostra o dele |
| widget de viagem que liga sozinho e tapa o mapa | Nada liga sozinho |
| anúncios em tela cheia | Sem anúncio |

## A aba "Tabela de horários"

Dado real: os intervalos publicados em `frequencies.txt` (485 das 494 linhas), agregados por hora e por tipo de dia (dia útil, sábado, domingo). Mostra "a cada ~N min" para a hora atual e as faixas do dia, no mesmo formato que o site do próprio Lá vem usa ("Intervalo médio entre os ônibus em cada horário").

## Revisão 4.1: o mapa estava pesado demais

O Brian achou o mapa denso. As camadas somavam: todas as paradas do percurso inteiro de cada linha nos dois sentidos, todos os ônibus das linhas acompanhadas na cidade toda, as rotas de volta e o fundo do OpenStreetMap, muito mais carregado que o do Google.

Regra nova: o mapa mostra o que responde à pergunta do momento, e o resto aparece quando há motivo.

| Situação | O que aparece |
|---|---|
| Padrão | rota de cada linha só no sentido que você pega; a parada que você usaria (uma placa por linha); os próximos três ônibus a caminho dela |
| Linha em foco (toque na bolinha) | só aquela linha, com todas as paradas do sentido e todos os ônibus nele |
| Parada aberta | a parada e os ônibus que estão vindo para ela, até oito |
| Zoom 15 | paradas num raio de 500 m, como pontinhos |
| Zoom 16 ou mais | todas as paradas da tela, como pontinhos |

Os ônibus encolheram (18 a 30 px em vez de 22 a 38, ainda crescendo com o zoom) e o fundo foi lavado até recuar. O ruído que sobra no fundo (cruz de hospital, âncora, rota de barca) vem do estilo padrão do OSM e só some com os tiles próprios da Fase 2.

## Revisão 4.2: nosso próprio mapa de fundo

O fundo do OpenStreetMap era a maior fonte de peso visual que sobrava, e nenhum filtro de cor tira ícone de hospital, âncora ou rota de barca. Além disso, a política de uso dos servidores do OSM proíbe apps com tráfego real, então esse passo era necessário de qualquer forma.

Agora o cliente desenha o fundo a partir de tiles vetoriais próprios, gerados por `busones base build` a partir de um recorte do Protomaps (OpenStreetMap processado, ODbL):

- **O que fica**: água, áreas verdes, praias, ruas em três classes (expressas em amarelo-claro, principais e locais em branco) e nomes de avenidas e de bairros.
- **O que sai**: prédios, pontos de interesse, ferrovias, barcas, limites administrativos, uso do solo comercial e residencial.
- **Tamanho**: um tile do Centro no zoom 15 caiu de 45 KB para 4,4 KB; a mediana é 0,4 KB, contra 15 a 30 KB de um PNG do OSM. O mapa inteiro do município tem 5,8 MB gzip, em 4.411 tiles.
- **Desenho**: cada camada vira um Path2D uma vez, e é redesenhada com transformação ao arrastar. Os nomes são desenhados numa passada separada sobre a tela inteira, então nunca são cortados na borda de um tile, e um nome que sobreporia outro é pulado.

## Revisão 4.3: para onde o ônibus está indo

O Brian não conseguia ver para onde cada ônibus ia. O marcador era um círculo com um ônibus desenhado de frente, e a direção ficava numa seta minúscula na borda. É exatamente a queixa número um sobre o Lá vem, que nós tínhamos prometido resolver.

- **Ônibus**: selo com o número da linha, sempre na vertical para ser lido, e uma seta grande do lado de fora, no ponto em que a direção do ônibus sai do selo. É o marcador do bustimes.org. No zoom aberto, um ponto com a seta.
- **Rota**: setas brancas ao longo do traçado, a cada 90 px, no sentido em que a linha anda.
- O destino por extenso continua na pílula ("Sentido Terminal Deodoro") e no balão ao tocar no ônibus.

## Revisão 4.4: ícone do onibus-rj e local sem GPS

- **Ônibus no mapa**: a gota do onibus-rj (`drawTeardropPin` do rio-map): corpo sobre o ônibus, ponta de 22 px na direção em que ele vai, contorno e ponto brancos; alfinete comum quando não há direção; cinza `#94a3b8` sem sinal; esmaecido com sentido não confirmado. Cresce de leve com o zoom (0,85 a 1,25).
- **Onde você está**: o botão com alfinete no topo (onde ficava "Rio de Janeiro") abre "Onde você está?": usar o GPS, escolher no mapa (alfinete fixo no centro, "Estou aqui") ou buscar bairro ou parada pelo nome, sem acento. Os nomes vêm dos nossos dados (1.011 bairros e 7.694 paradas), sem serviço de geocodificação. O local escolhido fica salvo e é usado ao abrir, sem pedir GPS, como no onibus-rj.
- **Você no mapa**: ponto azul quando vem do GPS; alfinete vermelho `#dc2626` (o do onibus-rj) quando foi escolhido.

## Revisão 4.5: local por arraste ou endereço; menos botões

- **Arrastar o alfinete** direto no mapa, sem entrar em modo nenhum; **segurar o dedo** num ponto do mapa também diz "estou aqui". O nome vem do endereço reverso ("Avenida Presidente Antônio Carlos, Centro").
- **Endereço com número**: a busca do painel "Onde você está?" mostra bairros e paradas enquanto você digita e oferece "Buscar endereço" para rua e número. O endereço passa pelo backend (`/api/v1/geocode` e `/api/v1/reverse`), que consulta o Nominatim como o onibus-rj fazia no worker, com cache de 7 dias e no máximo um pedido por segundo, e nunca a cada tecla, como a política de uso do Nominatim exige.
- **Pílulas removidas** (o Brian achou que não faziam sentido). As ações delas foram para o menu do selo da linha, no molde do menu do onibus-rj: ver só esta linha, escolher o sentido ("Indo para …"), horários na parada e remover linha.
- **Botão verde de busca removido**: repetia a lupa da barra do topo.

## Revisão 4.6: trilho embaixo, ações no ônibus, foco múltiplo

- **Trilho de linhas no canto inferior esquerdo**, empilhado de baixo para cima como no onibus-rj (a primeira linha fica mais perto do polegar). Some quando a folha de horários ou o painel de local cobrem a área.
- **Tocar num ônibus** mostra o balão de informação (linha, carro, idade do GPS, velocidade, sentido e chegada), ajustado para caber na tela. Chegou a ter botões Fixar, Focar e Remover, retirados a pedido do Brian.
- **Foco em várias linhas**: o foco virou um conjunto. No menu do selo, "Focar nesta linha" e "Tirar o foco desta linha", mais "Mostrar todas as linhas" quando houver foco.
- **Menu do selo sem "Horários em …"**, a pedido. Os horários continuam ao tocar na parada.
- **Paradas só das suas linhas**: a camada com todas as paradas da região saiu. Aparecem só as paradas das linhas que você acompanha, e com foco, só as das linhas em foco.

## Revisão 4.7: remover todas as linhas

Um selo vermelho a mais no fim do trilho, com ×, remove todas as linhas de uma vez, direto. Chegou a mostrar um aviso "Desfazer", retirado a pedido do Brian.

## Revisão 4.8: busca começa pelas linhas úteis perto de você

Ao abrir a busca de linha, depois de "Suas linhas", vêm as linhas a até 600 m ordenadas pelo tempo até você estar dentro do ônibus: caminhada até a parada mais próxima da linha (4,5 km/h) mais a espera média, que é metade do intervalo da hora atual. Uma linha a cada 3 min a 400 m (~7 min) fica acima de uma a cada 20 min na porta (~11 min), e linha frequente longe não aparece. Cada linha mostra "a cada ~N min · M min a pé"; linha sem serviço na hora vai para o fim. Os intervalos vêm de `dist/freq.json` (485 linhas, 19,8 KB gzip), carregado só quando a busca abre.

## Revisão 4.9: selos de largura fixa nas listas

Nas listas (horários da parada, busca, tabela de horários) todo selo tem 58 px de largura, e os destinos começam na mesma coluna. A fonte diminui com o código: 14 px até 4 caracteres, 13 px com 5 (SV777), 11,5 px com 6 e 10,5 px com 7 (LECD142).

## Revisão 4.10: cor oficial fixa por linha

A paleta do onibus-rj, sorteada pela posição da linha no trilho, saiu: o Brian achou as cores feias, e a cor de uma linha mudava quando outra era removida. Cada linha agora tem sempre a mesma cor, a oficial do GTFS da SMTR (`route_color` e `route_text_color`), que é a faixa da região de operação na nova pintura dos ônibus (Resolução SMTR 3870, de 2025): cinza para Grande Tijuca, Centro e Zona Sul; azul-escuro para Pavuna e Leopoldina; verde para Anchieta, Madureira e Méier; vermelho para a Ilha do Governador; rosa para Jacarepaguá; azul-claro para a Barra; laranja para Campo Grande e Guaratiba; marrom para Bangu e Realengo; roxo para Santa Cruz. Os executivos são azul-marinho e cada corredor do BRT tem a sua cor. É o que Google Maps, Moovit, Transit e Citymapper fazem com a cor da agência.

- **Selos** (trilho, listas, balão) usam a cor e a cor de texto oficiais, sem ajuste. As 10 linhas sem cor no GTFS e as linhas sem rota usam `#546E7A`.
- **Mapa** (rota, paradas e ônibus): a mesma cor, só mais escura (tema claro) ou mais clara (tema escuro) até ter contraste de 3:1 com o fundo. O cinza oficial sumiria entre as ruas do mapa claro, e o azul-marinho no escuro. No menu do selo, a cor como texto vai até 4,5:1.
- **Mesma cor, linhas diferentes**: linhas da mesma região têm a mesma cor. Quando duas linhas no mapa têm, cada ônibus delas leva o número ao lado, fora das etiquetas das paradas, como nos apps de cidades em que todo ônibus é da mesma cor.

## Revisão 4.11: o app é azul

A pedido do Brian, o verde do Lá vem deixou de ser a cor do app. A cor da marca é o azul `#1A73E8`, o mesmo do ponto do GPS: botão de pesquisa, ícone de local na barra, alfinete do local escolhido no mapa (antes vermelho), interruptores dos ajustes, botão "Estou aqui" e contorno de foco. O verde ficou só onde quer dizer "ao vivo" (a bolinha de "Tempo real", o sinal ao lado do tempo, "chegando"); o vermelho, só no × que remove todas as linhas e em "Remover linha". A aba se chama "Busones"; o ícone é um ônibus branco sobre um quadrado azul arredondado.

## Revisão 4.12: o local só muda quando você pede

A pedido do Brian, saíram o arraste livre do alfinete e o "segurar o dedo no mapa": era fácil mudar o local sem querer ao mexer no mapa. O local agora muda só pelo GPS, pela busca de bairro, parada ou endereço, ou por "Escolher no mapa", com o alfinete fixo no centro. O painel "Onde você está?" também perdeu a terceira linha, que repetia o local já escolhido sem fazer nada útil.

## Revisão 4.13: nove linhas, nove cores

As cores oficiais por região (revisão 4.10) saíram: linhas da mesma região ficavam iguais, e o Brian não gostou. Agora são no máximo 9 linhas acompanhadas e uma paleta de 9 cores no mesmo tom: o azul do app (`#1A73E8`), que é a primeira, e oito matizes gerados com a mesma claridade percebida dele (OKLCH L 0,57), escurecidos só o necessário para o número branco ter contraste de 4,5:1 ou mais. A Tableau 10 chegou a ser usada e saiu: misturava tons pastel e fortes, e metade das cores pedia letra preta. Cada linha pega a primeira cor livre ao ser adicionada e fica com ela até ser removida; remover uma linha não muda a cor das outras, e duas linhas suas nunca têm a mesma cor. A 10ª linha é recusada com um aviso. As cores ficam salvas no aparelho. Linhas que você não acompanha (por exemplo, na folha de uma parada) usam o azul-marinho, e quando várias estão no mapa o número aparece ao lado do ônibus. O GTFS continua exportando a cor oficial de cada linha, que não é mais usada no cliente.
